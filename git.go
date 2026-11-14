package main

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

type Workspace struct {
	Root     string
	MainRoot string
	Name     string
	Subdir   string
	Git      *GitState
	Secrets  []string
}

type GitState struct {
	Origin     string
	RootCommit string
	Branch     string
	Worktree   string
	Index      string
}

const (
	handoffRef  = "refs/cielago/handoff"
	worktreeRef = "refs/cielago/worktree"
	indexRef    = "refs/cielago/index"
	parkedRef   = "refs/cielago/parked"
)

var snapshotIdentity = []string{
	"GIT_AUTHOR_NAME=cielago", "GIT_AUTHOR_EMAIL=cielago@localhost",
	"GIT_COMMITTER_NAME=cielago", "GIT_COMMITTER_EMAIL=cielago@localhost",
}

func readWorkspace(ctx context.Context, cwd string, secretPatterns []string) (Workspace, error) {
	realCwd, err := filepath.EvalSymlinks(cwd)
	if err != nil {
		return Workspace{}, err
	}
	git := func(env []string, args ...string) (string, error) {
		out, err := run(ctx, realCwd, env, "git", args...)
		return strings.TrimSpace(string(out)), err
	}
	top, err := git(nil, "rev-parse", "--show-toplevel")
	if exitCode(err) == 128 {
		return Workspace{Root: realCwd, MainRoot: realCwd, Name: filepath.Base(realCwd), Subdir: "."}, nil
	}
	if err != nil {
		return Workspace{}, err
	}
	ws := Workspace{Root: top, Git: &GitState{}}
	if ws.Subdir, err = filepath.Rel(ws.Root, realCwd); err != nil {
		return Workspace{}, err
	}
	common, err := git(nil, "rev-parse", "--path-format=absolute", "--git-common-dir")
	if err != nil {
		return Workspace{}, err
	}
	if ws.MainRoot, err = filepath.EvalSymlinks(filepath.Dir(common)); err != nil {
		return Workspace{}, err
	}
	ws.Name = filepath.Base(ws.MainRoot)

	g := ws.Git
	if g.Origin, err = git(nil, "remote", "get-url", "origin"); err != nil && exitCode(err) != 2 {
		return Workspace{}, err
	}
	roots, err := git(nil, "rev-list", "--max-parents=0", "HEAD")
	if err != nil {
		return Workspace{}, fmt.Errorf("%s has no commits yet; make a first commit so it can be handed off", ws.Root)
	}
	g.RootCommit = slices.Min(strings.Fields(roots))
	if g.Branch, err = git(nil, "symbolic-ref", "--short", "-q", "HEAD"); err != nil && exitCode(err) != 1 {
		return Workspace{}, err
	}
	if g.Worktree, g.Index, err = snapshot(git); err != nil {
		return Workspace{}, err
	}
	if ws.Secrets, err = ignoredFiles(git, secretPatterns); err != nil {
		return Workspace{}, err
	}
	return ws, nil
}

func snapshot(git func(env []string, args ...string) (string, error)) (worktree, index string, err error) {
	indexTree, err := git(nil, "write-tree")
	if err != nil {
		return "", "", errors.New("the repository has unresolved merge conflicts; resolve them before handing off")
	}
	indexPath, err := git(nil, "rev-parse", "--path-format=absolute", "--git-path", "index")
	if err != nil {
		return "", "", err
	}
	tmp, err := os.CreateTemp("", "cielago-index-*")
	if err != nil {
		return "", "", err
	}
	tmp.Close()
	defer os.Remove(tmp.Name())
	if data, err := os.ReadFile(indexPath); err == nil {
		if err := os.WriteFile(tmp.Name(), data, 0o600); err != nil {
			return "", "", err
		}
	}
	env := []string{"GIT_INDEX_FILE=" + tmp.Name()}
	if _, err := git(env, "add", "-A"); err != nil {
		return "", "", err
	}
	worktreeTree, err := git(env, "write-tree")
	if err != nil {
		return "", "", err
	}
	if worktree, err = git(snapshotIdentity, "commit-tree", worktreeTree, "-p", "HEAD", "-m", "cielago: working tree"); err != nil {
		return "", "", err
	}
	if index, err = git(snapshotIdentity, "commit-tree", indexTree, "-p", "HEAD", "-m", "cielago: staging area"); err != nil {
		return "", "", err
	}
	return worktree, index, nil
}

func ignoredFiles(git func(env []string, args ...string) (string, error), patterns []string) ([]string, error) {
	if len(patterns) == 0 {
		return nil, nil
	}
	args := []string{"ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory", "--"}
	for _, p := range patterns {
		args = append(args, ":(glob)**/"+p)
	}
	out, err := git(nil, args...)
	return withoutDirs(splitNull([]byte(out))), err
}

func park(ctx context.Context, ws Workspace) error {
	_, err := run(ctx, ws.Root, nil, "git", "update-ref", parkedRef, ws.Git.Worktree)
	return err
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

func checkoutScript(repo, branch string) string {
	var s strings.Builder
	s.WriteString("set -e; cd " + quote(repo) + "; ")
	s.WriteString(`tree_now() { i=$(mktemp); cp "$(git rev-parse --git-path index)" "$i" 2>/dev/null || :; ` +
		`GIT_INDEX_FILE="$i" git add -A >/dev/null; GIT_INDEX_FILE="$i" git write-tree; rm -f "$i"; }; `)
	s.WriteString(`parked=; if [ -n "$(git status --porcelain)" ]; then ` +
		`parked=$(git rev-parse -q --verify '` + parkedRef + `^{tree}' || :); ` +
		`if [ -z "$parked" ] || [ "$(tree_now)" != "$parked" ]; then ` +
		`echo "$PWD has uncommitted changes made on this machine; commit or stash them there first" >&2; exit 3; fi; fi; `)
	if branch == "" {
		s.WriteString(`[ -n "$parked" ] && git clean -fdq; git checkout --quiet --force --detach ` + handoffRef + "; ")
	} else {
		ref := quote("refs/heads/" + branch)
		s.WriteString("if git rev-parse --verify --quiet " + ref + " >/dev/null; then " +
			"rc=0; git merge-base --is-ancestor " + ref + " " + handoffRef + " || rc=$?; " +
			"if [ $rc -eq 1 ]; then printf 'branch %s has commits this machine does not; pull them here first\\n' " + quote(branch) + " >&2; exit 4; fi; " +
			"[ $rc -eq 0 ]; fi; ")
		s.WriteString(`[ -n "$parked" ] && git clean -fdq; git checkout --quiet --force -B ` + quote(branch) + " " + handoffRef + "; ")
	}
	s.WriteString("git read-tree -m -u HEAD " + worktreeRef + "; git read-tree " + indexRef + "; ")
	s.WriteString("for r in " + worktreeRef + " " + indexRef + " " + parkedRef + "; do git update-ref -d $r 2>/dev/null || :; done")
	return s.String()
}

func syncGit(r *Remote, ws Workspace, remoteRepo string) error {
	push := []string{"push", "--quiet", "--force", "--no-verify", r.host + ":" + remoteRepo,
		"HEAD:" + handoffRef, ws.Git.Worktree + ":" + worktreeRef, ws.Git.Index + ":" + indexRef}
	if _, err := run(r.ctx, ws.Root, []string{"GIT_SSH_COMMAND=" + r.sshCommand()}, "git", push...); err != nil {
		return err
	}
	if _, err := r.sh(checkoutScript(remoteRepo, ws.Git.Branch)); err != nil {
		return err
	}
	return r.copy(Archive{Dir: ws.Root, Paths: ws.Secrets}, remoteRepo)
}
