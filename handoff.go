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
	Where     string
	ShowNote  error
	TabNote   error
	ParkNote  error
	NewFolder string
	From      string
	FromDir   string
	ToDir     string
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

type stepper struct {
	steps []Step
}

func (s *stepper) run(name string, fn func() error) error {
	start := time.Now()
	err := fn()
	s.steps = append(s.steps, Step{name, time.Since(start)})
	if err != nil {
		return fmt.Errorf("%s: %w", name, err)
	}
	return nil
}

func (h Handoff) Run() (Result, error) {
	ctx, cancel := context.WithTimeout(context.Background(), handoffBudget)
	defer cancel()
	var res Result
	var st stepper

	local, err := newLocal(ctx)
	if err != nil {
		return res, err
	}
	var remote *Remote
	var ws Workspace
	var where Placement
	err = st.run("inspect", func() (err error) {
		if ws, err = readWorkspace(local, h.Cwd, h.Host.Secrets); err != nil {
			return err
		}
		if err := refuseHome(ws); err != nil {
			return err
		}
		if remote, err = newRemote(ctx, h.Host.SSH); err != nil {
			return err
		}
		group, err := localGroup(ws.MainRoot, h.Host.LocalRoots)
		if err != nil {
			return err
		}
		where, err = place(remote, ws, h.Host.Roots, group, h.Host.Exclude)
		return err
	})
	if err != nil {
		res.Steps = st.steps
		return res, err
	}
	res.NewFolder = where.NewFolder
	res.From = local.Name()
	res.FromDir = h.Cwd
	remoteDir := filepath.Join(where.Root, ws.Subdir)
	res.ToDir = remoteDir

	launcher := newLauncher(h.Host, remote)
	var tab Tab
	undo := func(err error) error {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		remote.SetContext(cleanup)
		if tab.Pane != "" {
			if stopErr := stopSession(remote, h.SessionID); stopErr != nil {
				err = fmt.Errorf("%w; stopping it there also failed: %v", err, stopErr)
			}
		}
		if closeErr := launcher.Discard(tab); closeErr != nil {
			err = fmt.Errorf("%w; removing the new tab also failed: %v", err, closeErr)
		}
		return withFolderUndo(err, remote, where, h.HostName)
	}

	if err := st.run("sync", func() error { return syncProject(local, remote, ws, where, h.Host.Exclude) }); err != nil {
		res.Steps = st.steps
		if where.NewFolder == "" && ws.Git != nil {
			return res, err
		}
		return res, undo(err)
	}

	sidecar := ""
	if err := st.run("transcript", func() (err error) {
		sidecar, err = moveTranscript(local, h.TranscriptPath, ws, remote, where, remoteDir)
		return err
	}); err != nil {
		res.Steps = st.steps
		return res, undo(err)
	}

	launchedAt := time.Now()
	name := baseName(where.Root)
	if err := st.run("launch", func() (err error) {
		prompt := handoffPrompt(Prompt{From: local.Name(), To: h.HostName, OldDir: h.Cwd, NewDir: remoteDir, Git: ws.Git != nil, Note: h.Note, Sidecar: sidecar})
		command := []string{expandHome(h.Host.Claude, where.Home), "--resume", h.SessionID, "--remote-control", name, prompt}
		if tab, err = launcher.Start(where.Group, claudeTabLabel(name), remoteDir, command); err != nil {
			return err
		}
		res.ShownOn, res.ShowNote = launcher.Show(tab)
		res.Where = launcher.Where(where.Group)
		return nil
	}); err != nil {
		res.Steps = st.steps
		return res, undo(err)
	}

	if err := st.run("start", func() error { return waitForRemoteControl(remote, h.SessionID, launchedAt) }); err != nil {
		if trusted, trustErr := isTrusted(remote, remoteDir); trustErr == nil && !trusted {
			err = fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that there and hand off again", err, remoteDir)
		}
		res.Steps = st.steps
		return res, undo(err)
	}

	finish, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	local.SetContext(finish)
	remote.SetContext(finish)
	res.ParkNote = settle(local, ws, h.Host.Exclude)
	if h.Host.NewTabCommand != "" {
		res.TabNote = st.run("tab", func() error {
			return launcher.OpenProjectTab(tab, name, where.Root, strings.ReplaceAll(h.Host.NewTabCommand, "{repo}", quote(name)))
		})
	}
	res.Steps = st.steps
	return res, nil
}

func refuseHome(ws Workspace) error {
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	if ws.Git == nil && (ws.Root == home || ws.Root == "/") {
		return fmt.Errorf("%s is a whole home folder, not a project; start Claude in the project's folder to hand it off", ws.Root)
	}
	return nil
}

func syncProject(local *Local, remote *Remote, ws Workspace, where Placement, exclude []string) error {
	if ws.Git == nil {
		if err := mirrorFolder(local, ws.Root, remote, where.Root, exclude); err != nil {
			return err
		}
		return registerFolder(remote, where.Root, exclude)
	}
	if where.NewFolder != "" {
		if err := createClone(remote, where.NewFolder, ws.Git.Origin); err != nil {
			return err
		}
	} else if err := checkoutAccepts(remote, where.Root, ws); err != nil {
		return err
	}
	return pushGit(local, remote, ws, where.Root)
}

func settle(source Machine, ws Workspace, exclude []string) error {
	if ws.Git != nil {
		return park(source, ws.Root, ws)
	}
	return registerFolder(source, ws.Root, exclude)
}

func withFolderUndo(err error, target Machine, where Placement, targetName string) error {
	if where.NewFolder == "" {
		return fmt.Errorf("%w (%s on %s now has this session's files)", err, where.Root, targetName)
	}
	if rmErr := removeFolder(target, where.NewFolder); rmErr != nil {
		return fmt.Errorf("%w; removing the new folder %s also failed: %v", err, where.NewFolder, rmErr)
	}
	return fmt.Errorf("%w (cielago removed the folder it had just created at %s)", err, where.NewFolder)
}

func moveTranscript(from Machine, transcriptPath string, ws Workspace, to Machine, where Placement, toDir string) (sidecar string, err error) {
	slug, err := projectSlug(toDir)
	if err != nil {
		return "", err
	}
	toProject := where.Home + "/.claude/projects/" + slug
	dir, files, err := sessionFiles(from, transcriptPath)
	if err != nil {
		return "", err
	}
	if len(files) > 1 {
		out, err := sh(from, "[ -d "+quote(dir+"/"+files[1]+"/subagents")+" ] && echo yes || :")
		if err != nil {
			return "", err
		}
		if strings.TrimSpace(string(out)) == "yes" {
			sidecar = toProject + "/" + files[1]
		}
	}
	if err := transfer(from, dir, Archive{Paths: files}, to, toProject); err != nil {
		return "", err
	}
	fromHome, err := homeDir(from)
	if err != nil {
		return "", err
	}
	fromSlug, err := projectSlug(ws.MainRoot)
	if err != nil {
		return "", err
	}
	toSlug, err := projectSlug(where.Root)
	if err != nil {
		return "", err
	}
	local, err := newLocal(from.Context())
	if err != nil {
		return "", err
	}
	return sidecar, syncMemory(local, from, fromHome+"/.claude/projects/"+fromSlug+"/memory", to, where.Home+"/.claude/projects/"+toSlug+"/memory")
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
		if group, ok := groupUnder(real, projectRoot); ok {
			return group, nil
		}
	}
	return baseName(projectRoot), nil
}

func serverGroup(clone string, roots []string, home string) string {
	for _, root := range roots {
		if group, ok := groupUnder(expandHome(root, home), clone); ok {
			return group
		}
	}
	return baseName(filepath.Dir(clone))
}

func groupUnder(root, path string) (string, bool) {
	rel, err := filepath.Rel(root, path)
	if err != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, "../") {
		return "", false
	}
	return strings.Split(rel, string(filepath.Separator))[0], true
}

type Placement struct {
	Home      string
	Root      string
	Group     string
	NewFolder string
}

func place(target Machine, ws Workspace, roots []string, groupIfNew string, exclude []string) (Placement, error) {
	home, err := homeDir(target)
	if err != nil {
		return Placement{}, err
	}
	p := Placement{Home: home}
	if ws.Git != nil {
		clones, err := listClones(target, roots)
		if err != nil {
			return p, err
		}
		if p.Root, err = matchClone(clones, ws, target.Name()); err != nil {
			return p, err
		}
		if p.Root != "" {
			p.Group = serverGroup(p.Root, roots, home)
			return p, nil
		}
	}
	p.Group = groupIfNew
	p.Root = expandHome(roots[0], home) + "/" + groupIfNew + "/" + ws.Name
	f, err := folderState(target, p.Root)
	if err != nil {
		return p, err
	}
	switch {
	case f.State == "new":
		p.NewFolder = p.Root
	case ws.Git != nil && f.State == "git" && ws.Git.Origin == "" && f.Origin == "" && f.RootCommit == "":
	case ws.Git != nil:
		return p, fmt.Errorf("%s already exists on %s but isn't a clone of this repository", p.Root, target.Name())
	case f.State != "ours":
		return p, fmt.Errorf("%s already exists on %s and wasn't created by cielago; move it away or hand off from a folder with another name", p.Root, target.Name())
	default:
		return p, folderUnchanged(target, p.Root, f.Recorded, exclude)
	}
	return p, nil
}

type Clone struct {
	Dir        string
	Origin     string
	RootCommit string
	Inode      string
}

func homeDir(m Machine) (string, error) {
	out, err := sh(m, `printf '%s' "$HOME"`)
	return string(out), err
}

func listClones(m Machine, roots []string) ([]Clone, error) {
	var script strings.Builder
	for _, root := range roots {
		dir := quote(root)
		if rest, ok := strings.CutPrefix(root, "~/"); ok {
			dir = `"$HOME"/` + quote(rest)
		}
		script.WriteString(`find ` + dir + ` -maxdepth 3 -name .git -type d -prune -print 2>/dev/null | while IFS= read -r g; do ` +
			`d=$(cd "${g%/.git}" && pwd -P); ` +
			`printf '%s\t%s\t%s\t%s\n' "$d" "$(git -C "$d" remote get-url origin 2>/dev/null)" "$(git -C "$d" rev-list --max-parents=0 HEAD 2>/dev/null | sort | head -n 1)" "$(ls -di "$d" | awk '{print $1}')"; done; `)
	}
	script.WriteString("true")
	out, err := sh(m, script.String())
	if err != nil {
		return nil, err
	}
	var clones []Clone
	seen := map[string]bool{}
	for _, line := range strings.Split(strings.TrimRight(string(out), "\n"), "\n") {
		fields := strings.Split(line, "\t")
		if len(fields) == 4 && !seen[fields[3]] {
			seen[fields[3]] = true
			clones = append(clones, Clone{Dir: fields[0], Origin: fields[1], RootCommit: fields[2], Inode: fields[3]})
		}
	}
	return clones, nil
}

func matchClone(clones []Clone, ws Workspace, host string) (string, error) {
	same := func(c Clone) bool {
		if ws.Git.Origin != "" {
			return c.Origin != "" && normalizeRemote(c.Origin) == normalizeRemote(ws.Git.Origin)
		}
		return c.Origin == "" && ws.Git.RootCommit != "" && c.RootCommit == ws.Git.RootCommit
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

func createClone(m Machine, dir, origin string) error {
	script := "set -e; mkdir -p " + quote(filepath.Dir(dir)) + "; mkdir " + quote(dir) + "; trap " + quote("rm -rf -- "+quote(dir)) + " EXIT; git init --quiet " + quote(dir) + "; "
	if origin != "" {
		script += "git -C " + quote(dir) + " remote add origin " + quote(origin) + "; "
	}
	script += "trap - EXIT"
	_, err := sh(m, script)
	return err
}

func removeFolder(m Machine, dir string) error {
	_, err := sh(m, "rm -rf -- "+quote(dir))
	return err
}
