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
	Note           string
}

type Step struct {
	Name string
	Took time.Duration
}

type Result struct {
	Steps     []Step
	ShownOn   []string
	RexNote   error
	TabNote   error
	ParkNote  error
	NewFolder string
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
	var remoteHome, remoteRoot, remoteDir, group string
	err := step("inspect", func() (err error) {
		if ws, err = readWorkspace(ctx, h.Cwd, h.Host.Secrets); err != nil {
			return err
		}
		if remote, err = newRemote(ctx, h.Host.SSH); err != nil {
			return err
		}
		if ws.Git != nil {
			var clones []Clone
			if remoteHome, clones, err = listClones(remote, h.Host.Roots); err != nil {
				return err
			}
			if remoteRoot, err = matchClone(clones, ws, remote.host); err != nil {
				return err
			}
		} else if remoteHome, err = remoteHomeDir(remote); err != nil {
			return err
		}
		if remoteRoot != "" {
			group = serverGroup(remoteRoot, h.Host.Roots, remoteHome)
		} else {
			if group, err = localGroup(ws.MainRoot, h.Host.LocalRoots); err != nil {
				return err
			}
			remoteRoot = expandHome(h.Host.Roots[0], remoteHome) + "/" + group + "/" + ws.Name
			exists, err := remoteExists(remote, remoteRoot)
			if err != nil {
				return err
			}
			if ws.Git != nil && exists {
				return fmt.Errorf("%s already exists on %s but isn't a clone of this repository", remoteRoot, remote.host)
			}
			if !exists {
				res.NewFolder = remoteRoot
			}
		}
		remoteDir = filepath.Join(remoteRoot, ws.Subdir)
		return nil
	})
	if err != nil {
		return res, err
	}

	var tab RexTab
	name := filepath.Base(remoteRoot)
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
		if res.NewFolder == "" {
			return fmt.Errorf("%w (%s on %s now has this session's files)", err, remoteRoot, h.HostName)
		}
		if rmErr := removeFolder(remote, res.NewFolder); rmErr != nil {
			return fmt.Errorf("%w; removing the new folder %s also failed: %v", err, res.NewFolder, rmErr)
		}
		return fmt.Errorf("%w (cielago removed the folder it had just created at %s)", err, res.NewFolder)
	}

	if err := step("sync", func() error {
		if ws.Git == nil {
			return remote.copy(Archive{Dir: ws.Root, Paths: []string{"."}, Exclude: h.Host.Exclude}, remoteRoot)
		}
		if res.NewFolder != "" {
			if err := createClone(remote, res.NewFolder, ws.Git.Origin); err != nil {
				return err
			}
		}
		return syncGit(remote, ws, remoteRoot)
	}); err != nil {
		if res.NewFolder == "" && ws.Git != nil {
			return res, err
		}
		return res, undo(err)
	}

	sidecar := ""
	if err := step("transcript", func() error {
		slug, err := projectSlug(remoteDir)
		if err != nil {
			return err
		}
		remoteProject := remoteHome + "/.claude/projects/" + slug
		dir, files, err := transcriptFiles(h.TranscriptPath)
		if err != nil {
			return err
		}
		if len(files) > 1 {
			sidecar = remoteProject + "/" + files[1]
		}
		if err := remote.copy(Archive{Dir: dir, Paths: files}, remoteProject); err != nil {
			return err
		}
		return syncMemory(remote, filepath.Join(dir, "memory"), remoteProject+"/memory")
	}); err != nil {
		return res, undo(err)
	}

	launchedAt := time.Now()
	if err := step("launch", func() error {
		from, err := os.Hostname()
		if err != nil {
			return err
		}
		prompt := handoffPrompt(Prompt{
			From: strings.TrimSuffix(from, ".local"), To: h.HostName, OldDir: h.Cwd, NewDir: remoteDir,
			Git: ws.Git != nil, Note: h.Note, Sidecar: sidecar,
		})
		launch.Command = []string{expandHome(h.Host.Claude, remoteHome), "--resume", h.SessionID, "--remote-control", name, prompt}
		if tab, err = launch.start(); err != nil {
			return err
		}
		res.ShownOn, res.RexNote = launch.show(tab)
		return nil
	}); err != nil {
		return res, undo(err)
	}

	if err := step("start", func() error { return waitForRemoteControl(remote, h.SessionID, launchedAt) }); err != nil {
		if trusted, trustErr := isTrusted(remote, remoteDir); trustErr == nil && !trusted {
			err = fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that in Rex and hand off again", err, remoteDir)
		}
		return res, undo(err)
	}

	finish, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	remote.ctx = finish
	if ws.Git != nil {
		res.ParkNote = park(finish, ws)
	}
	if h.Host.NewTabCommand != "" {
		res.TabNote = step("tab", func() error {
			return launch.openProjectTab(tab, remoteRoot, strings.ReplaceAll(h.Host.NewTabCommand, "{repo}", quote(name)))
		})
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

func localGroup(projectRoot string, localRoots []string) (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	for _, root := range localRoots {
		real, err := filepath.EvalSymlinks(expandHome(root, home))
		if os.IsNotExist(err) {
			continue
		}
		if err != nil {
			return "", fmt.Errorf("local_roots: %w", err)
		}
		rel, err := filepath.Rel(real, projectRoot)
		if err != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, "../") {
			continue
		}
		return strings.Split(rel, string(filepath.Separator))[0], nil
	}
	return filepath.Base(projectRoot), nil
}

func serverGroup(clone string, roots []string, home string) string {
	for _, root := range roots {
		rel, err := filepath.Rel(expandHome(root, home), clone)
		if err == nil && rel != "." && rel != ".." && !strings.HasPrefix(rel, "../") {
			return strings.Split(rel, string(filepath.Separator))[0]
		}
	}
	return filepath.Base(filepath.Dir(clone))
}

type Clone struct {
	Dir        string
	Origin     string
	RootCommit string
}

func remoteHomeDir(r *Remote) (string, error) {
	out, err := r.sh(`printf '%s' "$HOME"`)
	return string(out), err
}

func remoteExists(r *Remote, path string) (bool, error) {
	out, err := r.sh("[ -e " + quote(path) + " ] && echo yes || :")
	return strings.TrimSpace(string(out)) == "yes", err
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
		if ws.Git.Origin != "" {
			return c.Origin != "" && normalizeRemote(c.Origin) == normalizeRemote(ws.Git.Origin)
		}
		return c.Origin == "" && c.RootCommit == ws.Git.RootCommit
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
	script := "set -e; trap " + quote("rm -rf -- "+quote(dir)) + " EXIT; git init --quiet " + quote(dir) + "; "
	if origin != "" {
		script += "git -C " + quote(dir) + " remote add origin " + quote(origin) + "; "
	}
	script += "trap - EXIT"
	_, err := r.sh(script)
	return err
}

func removeFolder(r *Remote, dir string) error {
	_, err := r.sh("rm -rf -- " + quote(dir))
	return err
}
