package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const handoffBudget = 80 * time.Second

type Handoff struct {
	HostName       string
	Host           Host
	SessionID      string
	TranscriptPath string
	Cwd            string
}

type Step struct {
	Name string
	Took time.Duration
}

func summarize(steps []Step) string {
	var total time.Duration
	parts := make([]string, len(steps))
	for i, s := range steps {
		total += s.Took
		parts[i] = fmt.Sprintf("%s %.1fs", s.Name, s.Took.Seconds())
	}
	return fmt.Sprintf("%.1fs (%s)", total.Seconds(), strings.Join(parts, ", "))
}

func (h Handoff) Run() ([]Step, error) {
	ctx, cancel := context.WithTimeout(context.Background(), handoffBudget)
	defer cancel()

	var steps []Step
	step := func(name string, fn func() error) error {
		start := time.Now()
		err := fn()
		steps = append(steps, Step{name, time.Since(start)})
		if err != nil {
			return fmt.Errorf("%s: %w", name, err)
		}
		return nil
	}

	var ws Workspace
	var remote *Remote
	var remoteHome, remoteRepo, remoteDir string
	err := step("inspect", func() (err error) {
		if ws, err = readWorkspace(ctx, h.Cwd, h.Host.Secrets); err != nil {
			return err
		}
		if remote, err = newRemote(ctx, h.Host.SSH); err != nil {
			return err
		}
		if remoteHome, remoteRepo, err = findRemoteRepo(remote, h.Host.Roots, ws.Origin); err != nil {
			return err
		}
		remoteDir = filepath.Join(remoteRepo, ws.Subdir)
		return checkTrusted(remote, remoteDir)
	})
	if err != nil {
		return steps, err
	}

	if err := step("sync", func() error { return syncWorkspace(remote, ws, remoteRepo) }); err != nil {
		return steps, err
	}
	leftChanged := func(err error) error {
		return fmt.Errorf("%w (%s on %s now has this branch and these changes checked out)", err, remoteRepo, h.HostName)
	}

	if err := step("transcript", func() error {
		slug, err := projectSlug(remoteDir)
		if err != nil {
			return err
		}
		dir, files, err := transcriptFiles(h.TranscriptPath)
		if err != nil {
			return err
		}
		return remote.copyFiles(dir, files, remoteHome+"/.claude/projects/"+slug)
	}); err != nil {
		return steps, leftChanged(err)
	}

	launchedAt := time.Now()
	if err := step("launch", func() error {
		from, err := os.Hostname()
		if err != nil {
			return err
		}
		name := filepath.Base(remoteRepo)
		prompt := handoffPrompt(strings.TrimSuffix(from, ".local"), h.HostName, h.Cwd, remoteDir)
		return Launch{
			Host:    h.Host,
			Remote:  remote,
			Name:    name,
			Dir:     remoteDir,
			Command: []string{expandHome(h.Host.Claude, remoteHome), "--resume", h.SessionID, "--remote-control", name, prompt},
		}.start()
	}); err != nil {
		return steps, leftChanged(err)
	}

	if err := step("start", func() error { return waitForRemoteControl(remote, h.SessionID, launchedAt) }); err != nil {
		cleanup, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		remote.ctx = cleanup
		if stopErr := stopRemoteSession(remote, h.SessionID); stopErr != nil {
			err = fmt.Errorf("%w; stopping it there also failed: %v", err, stopErr)
		}
		return steps, leftChanged(err)
	}
	return steps, nil
}

func expandHome(path, home string) string {
	if path == "~" {
		return home
	}
	if rest, ok := strings.CutPrefix(path, "~/"); ok {
		return home + "/" + rest
	}
	return path
}

func findRemoteRepo(r *Remote, roots []string, origin string) (home, repo string, err error) {
	var script strings.Builder
	script.WriteString(`printf '%s\n' "$HOME"; `)
	for _, root := range roots {
		dir := quote(root)
		if rest, ok := strings.CutPrefix(root, "~/"); ok {
			dir = `"$HOME"/` + quote(rest)
		}
		script.WriteString(`find ` + dir + ` -maxdepth 3 -name .git -type d -prune -print 2>/dev/null | while IFS= read -r g; do ` +
			`d=$(cd "${g%/.git}" && pwd -P); printf '%s\t%s\n' "$d" "$(git -C "$d" remote get-url origin 2>/dev/null)"; done; `)
	}
	out, err := r.sh(script.String())
	if err != nil {
		return "", "", err
	}
	lines := strings.Split(strings.TrimRight(string(out), "\n"), "\n")
	home = lines[0]
	want := normalizeRemote(origin)
	var matches []string
	for _, line := range lines[1:] {
		dir, url, _ := strings.Cut(line, "\t")
		if url != "" && normalizeRemote(url) == want {
			matches = append(matches, dir)
		}
	}
	switch len(matches) {
	case 0:
		return "", "", fmt.Errorf("no clone of %s under %s on %s", want, strings.Join(roots, ", "), r.host)
	case 1:
		return home, matches[0], nil
	default:
		return "", "", fmt.Errorf("%d clones of %s on %s: %s", len(matches), want, r.host, strings.Join(matches, ", "))
	}
}

const handoffRef = "refs/cielago/handoff"

func syncWorkspace(r *Remote, ws Workspace, remoteRepo string) error {
	push := []string{"push", "--quiet", "--force", "--no-verify", r.host + ":" + remoteRepo, "HEAD:" + handoffRef}
	if _, err := run(r.ctx, ws.Root, []string{"GIT_SSH_COMMAND=" + r.sshCommand()}, "git", push...); err != nil {
		return err
	}
	if _, err := r.sh(checkoutScript(remoteRepo, ws.Branch, ws.Deleted)); err != nil {
		return err
	}
	return r.copyFiles(ws.Root, append(append([]string{}, ws.Changed...), ws.Secrets...), remoteRepo)
}

func checkoutScript(repo, branch string, deleted []string) string {
	script := "set -e; cd " + quote(repo) + "; " +
		`if [ -n "$(git status --porcelain)" ]; then echo "$PWD has uncommitted changes; commit or stash them there first" >&2; exit 3; fi; `
	if branch == "" {
		script += "git checkout --quiet --detach " + handoffRef + "; "
	} else {
		ref := quote("refs/heads/" + branch)
		script += "if git rev-parse --verify --quiet " + ref + " >/dev/null; then " +
			"rc=0; git merge-base --is-ancestor " + ref + " " + handoffRef + " || rc=$?; " +
			"if [ $rc -eq 1 ]; then printf 'branch %s has commits this machine does not; pull them here first\\n' " + quote(branch) + " >&2; exit 4; fi; " +
			"[ $rc -eq 0 ]; fi; " +
			"git checkout --quiet -B " + quote(branch) + " " + handoffRef + "; "
	}
	if len(deleted) > 0 {
		script += "rm -f -- " + quoteAll(deleted) + "; "
	}
	return script
}
