package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

func expandHome(path, home string) string {
	if path == "~" {
		return home
	}
	if rest, ok := strings.CutPrefix(path, "~/"); ok {
		return home + "/" + rest
	}
	return path
}

func localGroup(projectRoot string, localRoots []string) string {
	for _, root := range localRoots {
		if group, ok := groupUnderLocal(root, projectRoot); ok {
			return group
		}
	}
	return baseName(projectRoot)
}

func groupUnderLocal(root, path string) (string, bool) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", false
	}
	rootInfo, err := os.Stat(expandHome(root, home))
	if err != nil {
		return "", false
	}
	for child, parent := path, filepath.Dir(path); child != parent; child, parent = parent, filepath.Dir(parent) {
		if info, err := os.Stat(parent); err == nil && os.SameFile(info, rootInfo) {
			return filepath.Base(child), true
		}
	}
	return "", false
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
		return p, nil
	case ws.Git != nil:
		return p, fmt.Errorf("%s already exists on %s but isn't a clone of this repository", p.Root, target.Name())
	case f.State != "ours":
		return p, fmt.Errorf("%s already exists on %s and wasn't created by magi; move it away or hand off from a folder with another name", p.Root, target.Name())
	default:
		return p, folderUnchanged(target, p.Root, f.Recorded, exclude)
	}
	return p, nil
}

type Clone struct {
	Dir        string
	Origin     string
	OriginKey  string
	RootCommit string
}

func homeDir(m Machine) (string, error) {
	out, err := sh(m, `printf '%s' "$HOME"`)
	return string(out), err
}

func listClones(m Machine, roots []string) ([]Clone, error) {
	out, err := sh(m, clonesScript, roots...)
	if err != nil {
		return nil, err
	}
	var rows [][]string
	var origins []string
	seen := map[string]bool{}
	for _, line := range strings.Split(strings.TrimRight(string(out), "\n"), "\n") {
		fields := strings.Split(line, "\t")
		if len(fields) == 3 && !seen[fields[0]] {
			seen[fields[0]] = true
			rows = append(rows, fields)
			if fields[1] != "" {
				origins = append(origins, fields[1])
			}
		}
	}
	aliases, err := resolveAliases(m, origins)
	if err != nil {
		return nil, err
	}
	var clones []Clone
	for _, f := range rows {
		c := Clone{Dir: f[0], Origin: f[1], RootCommit: f[2]}
		if f[1] != "" {
			c.OriginKey = originKey(f[1], aliases)
		}
		clones = append(clones, c)
	}
	return clones, nil
}

func sameRepository(c Clone, ws Workspace) bool {
	if ws.Git.Origin != "" {
		return c.Origin != "" && (c.OriginKey == ws.Git.OriginKey || normalizeRemote(c.Origin) == normalizeRemote(ws.Git.Origin))
	}
	return c.Origin == "" && ws.Git.RootCommit != "" && c.RootCommit == ws.Git.RootCommit
}

func matchClone(clones []Clone, ws Workspace, host string) (string, error) {
	var matches []string
	for _, c := range clones {
		if sameRepository(c, ws) {
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

var errFolderAppeared = errors.New("the folder appeared meanwhile; move it away or hand off again")

func claimNewFolder(m Machine, dir string) error {
	_, err := folder(m, "claim", dir)
	if exitCode(err) == exitExists {
		return fmt.Errorf("%s on %s: %w", dir, m.Name(), errFolderAppeared)
	}
	return err
}

func createClone(m Machine, dir, origin string) error {
	if err := claimNewFolder(m, dir); err != nil {
		return err
	}
	return initRepo(m, dir, origin)
}

func initRepo(m Machine, dir, origin string) error {
	if _, err := sh(m, `mkdir -p -- "$(dirname -- "$1")" && git init --quiet "$1" && if [ -n "$2" ]; then git -C "$1" remote add origin "$2"; fi`, dir, origin); err != nil {
		return err
	}
	if origin != "" {
		prefetchOrigin(m, dir)
	}
	return nil
}

const prefetchLimit = 30 * time.Second

func prefetchOrigin(m Machine, dir string) {
	ctx, cancel := context.WithTimeout(m.Context(), prefetchLimit)
	defer cancel()
	sh(m.With(ctx), `GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=10' git -C "$1" -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=10 fetch --quiet origin`, dir)
}

func removeFolder(m Machine, dir string) error {
	_, err := sh(m, `rm -rf -- "$1"`, dir)
	return err
}
