package main

import (
	"context"
	"fmt"
	"net/url"
	"path/filepath"
	"slices"
	"strings"
)

type Workspace struct {
	Root       string
	MainRoot   string
	Name       string
	Subdir     string
	Origin     string
	RootCommit string
	Branch     string
	Changed    []string
	Deleted    []string
	Secrets    []string
}

func readWorkspace(ctx context.Context, cwd string, secretPatterns []string) (Workspace, error) {
	git := func(args ...string) ([]byte, error) {
		return run(ctx, cwd, nil, "git", args...)
	}
	root, err := git("rev-parse", "--show-toplevel")
	if err != nil {
		return Workspace{}, fmt.Errorf("%s is not inside a git repository", cwd)
	}
	ws := Workspace{Root: strings.TrimSpace(string(root))}
	realCwd, err := filepath.EvalSymlinks(cwd)
	if err != nil {
		return Workspace{}, err
	}
	if ws.Subdir, err = filepath.Rel(ws.Root, realCwd); err != nil {
		return Workspace{}, err
	}
	origin, err := git("remote", "get-url", "origin")
	if err != nil && exitCode(err) != 2 {
		return Workspace{}, err
	}
	ws.Origin = strings.TrimSpace(string(origin))
	roots, err := git("rev-list", "--max-parents=0", "HEAD")
	if err != nil {
		return Workspace{}, fmt.Errorf("%s has no commits yet; make a first commit so it can be handed off", ws.Root)
	}
	ws.RootCommit = slices.Min(strings.Fields(string(roots)))
	common, err := git("rev-parse", "--path-format=absolute", "--git-common-dir")
	if err != nil {
		return Workspace{}, err
	}
	if ws.MainRoot, err = filepath.EvalSymlinks(filepath.Dir(strings.TrimSpace(string(common)))); err != nil {
		return Workspace{}, err
	}
	ws.Name = filepath.Base(ws.MainRoot)
	branch, err := git("symbolic-ref", "--short", "-q", "HEAD")
	if err != nil && exitCode(err) != 1 {
		return Workspace{}, err
	}
	ws.Branch = strings.TrimSpace(string(branch))

	list := func(args ...string) ([]string, error) {
		out, err := git(args...)
		return splitNull(out), err
	}
	modified, err := list("diff", "--name-only", "--no-renames", "--ignore-submodules=all", "--diff-filter=d", "-z", "HEAD")
	if err != nil {
		return Workspace{}, err
	}
	untracked, err := list("ls-files", "-z", "--others", "--exclude-standard")
	if err != nil {
		return Workspace{}, err
	}
	ws.Changed = append(modified, withoutDirs(untracked)...)
	if ws.Deleted, err = list("diff", "--name-only", "--no-renames", "--ignore-submodules=all", "--diff-filter=D", "-z", "HEAD"); err != nil {
		return Workspace{}, err
	}
	if len(secretPatterns) == 0 {
		return ws, nil
	}
	args := []string{"ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory", "--"}
	for _, p := range secretPatterns {
		args = append(args, ":(glob)**/"+p)
	}
	ignored, err := list(args...)
	ws.Secrets = withoutDirs(ignored)
	return ws, err
}

func withoutDirs(paths []string) []string {
	var files []string
	for _, p := range paths {
		if !strings.HasSuffix(p, "/") {
			files = append(files, p)
		}
	}
	return files
}

func splitNull(out []byte) []string {
	var items []string
	for _, item := range strings.Split(string(out), "\x00") {
		if item != "" {
			items = append(items, item)
		}
	}
	return items
}

func normalizeRemote(raw string) string {
	s := strings.TrimSuffix(strings.TrimSuffix(strings.TrimSpace(raw), "/"), ".git")
	if strings.Contains(s, "://") {
		u, err := url.Parse(s)
		if err != nil {
			return s
		}
		return strings.ToLower(u.Hostname()) + "/" + strings.TrimPrefix(u.Path, "/")
	}
	if at := strings.Index(s, "@"); at >= 0 {
		s = s[at+1:]
	}
	host, path, found := strings.Cut(s, ":")
	if !found {
		return s
	}
	return strings.ToLower(host) + "/" + strings.TrimPrefix(path, "/")
}
