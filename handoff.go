package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const handoffBudget = 110 * time.Second

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

type Result struct {
	Steps    []Step
	ShownOn  []string
	RexNote  error
	NewClone string
}

func (r Result) Summary() string {
	var total time.Duration
	parts := make([]string, len(r.Steps))
	for i, s := range r.Steps {
		total += s.Took
		parts[i] = fmt.Sprintf("%s %.1fs", s.Name, s.Took.Seconds())
	}
	return fmt.Sprintf("%.1fs (%s)", total.Seconds(), strings.Join(parts, ", "))
}

func (h Handoff) Run() (Result, error) {
	ctx, cancel := context.WithTimeout(context.Background(), handoffBudget)
	defer cancel()

	var res Result
	step := func(name string, fn func() error) error {
		start := time.Now()
		err := fn()
		res.Steps = append(res.Steps, Step{name, time.Since(start)})
		if err != nil {
			return fmt.Errorf("%s: %w", name, err)
		}
		return nil
	}

	var ws Workspace
	var remote *Remote
	var remoteHome, remoteRepo, remoteDir, group string
	var trusted bool
	err := step("inspect", func() (err error) {
		if ws, err = readWorkspace(ctx, h.Cwd, h.Host.Secrets); err != nil {
			return err
		}
		if group, err = localGroup(ws.Root, h.Host.LocalRoots); err != nil {
			return err
		}
		if remote, err = newRemote(ctx, h.Host.SSH); err != nil {
			return err
		}
		var clones []Clone
		if remoteHome, clones, err = listClones(remote, h.Host.Roots); err != nil {
			return err
		}
		if remoteRepo, err = matchClone(clones, ws, remote.host); err != nil {
			return err
		}
		if remoteRepo == "" {
			res.NewClone = expandHome(h.Host.Roots[0], remoteHome) + "/" + group + "/" + filepath.Base(ws.Root)
			remoteRepo = res.NewClone
		}
		remoteDir = filepath.Join(remoteRepo, ws.Subdir)
		trusted, err = isTrusted(remote, remoteDir)
		return err
	})
	if err != nil {
		return res, err
	}

	var tab RexTab
	name := filepath.Base(remoteRepo)
	launch := Launch{Host: h.Host, Remote: remote, Group: group, Name: name, Dir: remoteDir}
	undo := func(err error) error {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		remote.ctx = cleanup
		if tab.Pane != "" {
			if stopErr := stopRemoteSession(remote, h.SessionID); stopErr != nil {
				err = fmt.Errorf("%w; stopping it there also failed: %v", err, stopErr)
			}
		}
		if closeErr := launch.discard(tab); closeErr != nil {
			err = fmt.Errorf("%w; removing the new Rex tab also failed: %v", err, closeErr)
		}
		if res.NewClone == "" {
			return fmt.Errorf("%w (%s on %s now has this branch and these changes checked out)", err, remoteRepo, h.HostName)
		}
		if rmErr := removeClone(remote, res.NewClone); rmErr != nil {
			return fmt.Errorf("%w; removing the new clone at %s also failed: %v", err, res.NewClone, rmErr)
		}
		return fmt.Errorf("%w (cielago removed the clone it had just created at %s)", err, res.NewClone)
	}

	if res.NewClone != "" {
		if err := step("clone", func() error { return createClone(remote, res.NewClone, ws.Origin) }); err != nil {
			return res, err
		}
	}
	if err := step("sync", func() error { return syncWorkspace(remote, ws, remoteRepo) }); err != nil {
		if res.NewClone == "" {
			return res, err
		}
		return res, undo(err)
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
		return res, undo(err)
	}

	launchedAt := time.Now()
	if err := step("launch", func() error {
		from, err := os.Hostname()
		if err != nil {
			return err
		}
		prompt := handoffPrompt(strings.TrimSuffix(from, ".local"), h.HostName, h.Cwd, remoteDir)
		launch.Command = []string{expandHome(h.Host.Claude, remoteHome), "--resume", h.SessionID, "--remote-control", name, prompt}
		if tab, err = launch.start(remoteRepo); err != nil {
			return err
		}
		res.ShownOn, res.RexNote = launch.show(tab)
		return nil
	}); err != nil {
		return res, undo(err)
	}

	if err := step("start", func() error { return waitForRemoteControl(remote, h.SessionID, launchedAt) }); err != nil {
		if !trusted {
			err = fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that in Rex and hand off again", err, remoteDir)
		}
		return res, undo(err)
	}

	if tab.Created && h.Host.NewTabCommand != "" {
		if err := step("tab", func() error {
			return launch.runInShell(tab, strings.ReplaceAll(h.Host.NewTabCommand, "{repo}", name))
		}); err != nil {
			return res, err
		}
	}
	return res, nil
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

func localGroup(repoRoot string, localRoots []string) (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	for _, root := range localRoots {
		real, err := filepath.EvalSymlinks(expandHome(root, home))
		if err != nil {
			return "", fmt.Errorf("local_roots: %w", err)
		}
		rel, err := filepath.Rel(real, repoRoot)
		if err != nil || rel == "." || strings.HasPrefix(rel, "..") {
			continue
		}
		return strings.Split(rel, string(filepath.Separator))[0], nil
	}
	return filepath.Base(repoRoot), nil
}

type Clone struct {
	Dir        string
	Origin     string
	RootCommit string
}

func listClones(r *Remote, roots []string) (home string, clones []Clone, err error) {
	var script strings.Builder
	script.WriteString(`printf '%s\n' "$HOME"; `)
	for _, root := range roots {
		dir := quote(root)
		if rest, ok := strings.CutPrefix(root, "~/"); ok {
			dir = `"$HOME"/` + quote(rest)
		}
		script.WriteString(`find ` + dir + ` -maxdepth 3 -name .git -type d -prune -print 2>/dev/null | while IFS= read -r g; do ` +
			`d=$(cd "${g%/.git}" && pwd -P); ` +
			`printf '%s\t%s\t%s\n' "$d" "$(git -C "$d" remote get-url origin 2>/dev/null)" "$(git -C "$d" rev-list --max-parents=0 HEAD 2>/dev/null | sort | head -n 1)"; done; `)
	}
	out, err := r.sh(script.String())
	if err != nil {
		return "", nil, err
	}
	lines := strings.Split(strings.TrimRight(string(out), "\n"), "\n")
	for _, line := range lines[1:] {
		fields := strings.Split(line, "\t")
		if len(fields) == 3 {
			clones = append(clones, Clone{Dir: fields[0], Origin: fields[1], RootCommit: fields[2]})
		}
	}
	return lines[0], clones, nil
}

func matchClone(clones []Clone, ws Workspace, host string) (string, error) {
	same := func(c Clone) bool {
		if ws.Origin != "" {
			return c.Origin != "" && normalizeRemote(c.Origin) == normalizeRemote(ws.Origin)
		}
		return c.Origin == "" && c.RootCommit == ws.RootCommit
	}
	var matches []string
	for _, c := range clones {
		if same(c) {
			matches = append(matches, c.Dir)
		}
	}
	switch len(matches) {
	case 0:
		return "", nil
	case 1:
		return matches[0], nil
	default:
		return "", fmt.Errorf("%d clones of this repository on %s: %s", len(matches), host, strings.Join(matches, ", "))
	}
}

func createClone(r *Remote, dir, origin string) error {
	script := "set -e; if [ -e " + quote(dir) + " ]; then echo " + quote(dir+" already exists but isn't a clone of this repository") + " >&2; exit 5; fi; " +
		"mkdir -p " + quote(dir) + "; git -C " + quote(dir) + " init --quiet; "
	if origin != "" {
		script += "git -C " + quote(dir) + " remote add origin " + quote(origin) + "; "
	}
	_, err := r.sh(script)
	return err
}

func removeClone(r *Remote, dir string) error {
	_, err := r.sh("rm -rf -- " + quote(dir))
	return err
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
