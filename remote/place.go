package main

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func groupOf(dir string, roots []string, home string) string {
	for _, root := range roots {
		rel, err := filepath.Rel(sh.ExpandHome(root, home), dir)
		if err == nil && rel != "." && rel != ".." && !strings.HasPrefix(rel, "../") {
			return strings.Split(rel, string(filepath.Separator))[0]
		}
	}
	return baseName(filepath.Dir(dir))
}

type Placement struct {
	Root      string
	Group     string
	NewFolder string
}

func place(target sh.Machine, ws Workspace, roots []string, groupIfNew string, exclude []string) (Placement, error) {
	var p Placement
	if ws.Git != nil {
		clones, err := listClones(target, roots)
		if err != nil {
			return p, err
		}
		if p.Root, err = matchClone(clones, ws, target.Name()); err != nil {
			return p, err
		}
		if p.Root != "" {
			p.Group = groupOf(p.Root, roots, target.Home())
			return p, nil
		}
	}
	p.Group = groupIfNew
	p.Root = sh.ExpandHome(roots[0], target.Home()) + "/" + groupIfNew + "/" + ws.Name
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

func listClones(m sh.Machine, roots []string) ([]Clone, error) {
	out, err := sh.Run(m, clonesScript, roots...)
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

func clonesOf(clones []Clone, ws Workspace) []string {
	var dirs []string
	for _, c := range clones {
		if sameRepository(c, ws) {
			dirs = append(dirs, c.Dir)
		}
	}
	return dirs
}

func matchClone(clones []Clone, ws Workspace, host string) (string, error) {
	matches := clonesOf(clones, ws)
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

func claimNewFolder(m sh.Machine, dir string) error {
	_, err := folder(m, "claim", dir)
	if sh.ExitCode(err) == sh.ExitExists {
		return fmt.Errorf("%s on %s: %w", dir, m.Name(), errFolderAppeared)
	}
	return err
}

func createClone(m sh.Machine, dir, origin string) error {
	if err := claimNewFolder(m, dir); err != nil {
		return err
	}
	return initRepo(m, dir, origin)
}

func initRepo(m sh.Machine, dir, origin string) error {
	if _, err := sh.Run(m, `mkdir -p -- "$(dirname -- "$1")" && git init --quiet "$1" && if [ -n "$2" ]; then git -C "$1" remote add origin "$2"; fi`, dir, origin); err != nil {
		return err
	}
	if origin != "" {
		prefetchOrigin(m, dir)
	}
	return nil
}

const prefetchLimit = 30 * time.Second

func prefetchOrigin(m sh.Machine, dir string) {
	ctx, cancel := context.WithTimeout(m.Context(), prefetchLimit)
	defer cancel()
	sh.Run(m.With(ctx), `GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=10' git -C "$1" -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=10 fetch --quiet origin`, dir)
}
