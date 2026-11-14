package main

import (
	"errors"
	"fmt"
	"net/url"
	"regexp"
	"strings"
)

var commitPattern = regexp.MustCompile(`^[0-9a-f]{40}([0-9a-f]{24})?$`)

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
	Head       string
	Worktree   string
	Index      string
}

const (
	handoffRef  = "refs/cielago/handoff"
	worktreeRef = "refs/cielago/worktree"
	indexRef    = "refs/cielago/index"
	parkedRef   = "refs/worktree/cielago/parked"
	outPrefix   = "refs/cielago/out/"
)

const (
	exitNoCommits  = 7
	exitConflicts  = 8
	exitInProgress = 9
)

func inspectScript(cwd string, secretPatterns []string) string {
	var pathspecs []string
	for _, p := range secretPatterns {
		pathspecs = append(pathspecs, quote(":(glob)**/"+p))
	}
	secrets := ":"
	if len(pathspecs) > 0 {
		secrets = "git ls-files -z --others --ignored --exclude-standard --directory -- " + strings.Join(pathspecs, " ") +
			` | tr '\0' '\n' | grep -v '/$' | while IFS= read -r f; do emit secret "$f"; done`
	}
	return `set -e; export LC_ALL=C; cd -- ` + quote(cwd) + `; emit() { printf '%s\0%s\0' "$1" "$2"; }; emit cwd "$(pwd -P)"; ` +
		`if ! top=$(git rev-parse --show-toplevel 2>&1); then case "$top" in *"not a git repository"*) emit plain 1; exit 0;; esac; echo "$top" >&2; exit 1; fi; ` +
		`emit root "$top"; common=$(git rev-parse --path-format=absolute --git-common-dir); emit mainroot "$(cd "$(dirname "$common")" && pwd -P)"; ` +
		`emit origin "$(git remote get-url origin 2>/dev/null || :)"; ` +
		`roots=$(git rev-list --max-parents=0 HEAD 2>/dev/null) || exit ` + fmt.Sprint(exitNoCommits) + `; ` +
		`emit rootcommit "$(printf '%s\n' $roots | sort | head -n 1)"; emit branch "$(git symbolic-ref --short -q HEAD || :)"; head=$(git rev-parse HEAD); emit head "$head"; ` +
		`[ -z "$(git ls-files --unmerged)" ] || exit ` + fmt.Sprint(exitConflicts) + `; ` +
		`for m in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD rebase-merge rebase-apply; do [ ! -e "$(git rev-parse --path-format=absolute --git-path $m)" ] || exit ` + fmt.Sprint(exitInProgress) + `; done; ` +
		`itree=$(git write-tree); d=$(mktemp -d); x=$(git rev-parse --path-format=absolute --git-path index); if [ -f "$x" ]; then cp "$x" "$d/index"; fi; ` +
		`trap 'rm -rf "$d"' EXIT; GIT_INDEX_FILE="$d/index" git add -A; wtree=$(GIT_INDEX_FILE="$d/index" git write-tree); ` +
		`export GIT_AUTHOR_NAME=cielago GIT_AUTHOR_EMAIL=cielago@localhost GIT_COMMITTER_NAME=cielago GIT_COMMITTER_EMAIL=cielago@localhost; ` +
		`wt=$(git commit-tree "$wtree" -p HEAD -m 'cielago: working tree'); idx=$(git commit-tree "$itree" -p HEAD -m 'cielago: staging area'); emit worktree "$wt"; emit index "$idx"; ` +
		secrets
}

func readWorkspace(m Machine, cwd string, secretPatterns []string) (Workspace, error) {
	out, err := sh(m, inspectScript(cwd, secretPatterns))
	switch exitCode(err) {
	case exitNoCommits:
		return Workspace{}, fmt.Errorf("%s has no commits yet; make a first commit so it can be handed off", cwd)
	case exitConflicts:
		return Workspace{}, errors.New("the repository has unresolved merge conflicts; resolve them before handing off")
	case exitInProgress:
		return Workspace{}, errors.New("a merge, rebase or cherry-pick is in progress; finish or abort it before handing off")
	}
	if err != nil {
		return Workspace{}, err
	}
	fields := strings.Split(string(out), "\x00")
	var ws Workspace
	var real string
	g := &GitState{}
	for i := 0; i+1 < len(fields); i += 2 {
		value := fields[i+1]
		switch fields[i] {
		case "cwd":
			real = value
		case "plain":
			return Workspace{Root: real, MainRoot: real, Name: baseName(real), Subdir: "."}, nil
		case "root":
			ws.Root = value
		case "mainroot":
			ws.MainRoot = value
		case "origin":
			g.Origin = value
		case "rootcommit":
			g.RootCommit = value
		case "branch":
			g.Branch = value
		case "head":
			g.Head = value
		case "worktree":
			g.Worktree = value
		case "index":
			g.Index = value
		case "secret":
			ws.Secrets = append(ws.Secrets, value)
		}
	}
	for _, id := range []string{g.Head, g.Worktree, g.Index} {
		if !commitPattern.MatchString(id) {
			return Workspace{}, fmt.Errorf("unexpected commit id %q from %s", id, m.Name())
		}
	}
	ws.Git = g
	ws.Name = baseName(ws.MainRoot)
	ws.Subdir = strings.TrimPrefix(strings.TrimPrefix(real, ws.Root), "/")
	if ws.Subdir == "" {
		ws.Subdir = "."
	}
	return ws, nil
}

func baseName(path string) string {
	return path[strings.LastIndex(path, "/")+1:]
}

func park(m Machine, ws Workspace) error {
	_, err := sh(m, "cd "+quote(ws.Root)+" && git update-ref "+parkedRef+" "+ws.Git.Worktree)
	return err
}

func checkoutScript(repo string, g GitState) string {
	var s strings.Builder
	s.WriteString("set -e; cd " + quote(repo) + "; H=" + quote(g.Head) + "; W=" + quote(g.Worktree) + "; I=" + quote(g.Index) + "; ")
	s.WriteString(`tree_now() { d=$(mktemp -d); x=$(git rev-parse --path-format=absolute --git-path index); if [ -f "$x" ]; then cp "$x" "$d/index"; fi; ` +
		`GIT_INDEX_FILE="$d/index" git add -A >/dev/null; GIT_INDEX_FILE="$d/index" git write-tree; rm -rf "$d"; }; `)
	s.WriteString(`force=; if [ -n "$(git status --porcelain --untracked-files=all --ignore-submodules=none)" ]; then ` +
		`now=$(tree_now); parked=$(git rev-parse -q --verify '` + parkedRef + `^{tree}' || :); ` +
		`if [ "$now" = "$parked" ] || [ "$now" = "$(git rev-parse "$W^{tree}")" ]; then force=1; else ` +
		`echo "$PWD has uncommitted changes made on this machine; commit or stash them there first" >&2; exit 3; fi; fi; `)
	s.WriteString(`git -c core.quotepath=false diff --name-only --diff-filter=A "$H" "$W" | while IFS= read -r p; do ` +
		`if [ -e "$p" ] && git check-ignore -q -- "$p" && [ "$(git hash-object -- "$p")" != "$(git rev-parse "$W:$p")" ]; then ` +
		`printf '%s exists here, is ignored, and would be overwritten; move it away first\n' "$p" >&2; exit 6; fi; done; `)
	if g.Branch == "" {
		s.WriteString(`git checkout --quiet ${force:+--force} --detach "$H"; `)
	} else {
		ref := quote("refs/heads/" + g.Branch)
		s.WriteString("if git rev-parse --verify --quiet " + ref + " >/dev/null; then " +
			`rc=0; git merge-base --is-ancestor ` + ref + ` "$H" || rc=$?; ` +
			"if [ $rc -eq 1 ]; then printf 'branch %s has commits this machine does not; pull them here first\\n' " + quote(g.Branch) + " >&2; exit 4; fi; " +
			"[ $rc -eq 0 ]; fi; ")
		s.WriteString(`git checkout --quiet ${force:+--force} -B ` + quote(g.Branch) + ` "$H"; `)
	}
	s.WriteString(`if [ -n "$force" ]; then git clean -fdq; fi; git read-tree -m -u HEAD "$W"; git read-tree "$I"; `)
	s.WriteString("for r in " + handoffRef + " " + worktreeRef + " " + indexRef + " " + parkedRef + "; do git update-ref -d $r; done")
	return s.String()
}

func pushGit(local *Local, remote *Remote, ws Workspace, remoteRepo string) error {
	refspecs := []string{"HEAD:" + handoffRef, ws.Git.Worktree + ":" + worktreeRef, ws.Git.Index + ":" + indexRef}
	push := "cd " + quote(ws.Root) + " && GIT_SSH_COMMAND=" + quote(remote.sshCommand()) +
		" git push --quiet --force --no-verify " + quote(remote.host+":"+remoteRepo) + " " + quoteAll(refspecs)
	if _, err := sh(local, push); err != nil {
		return err
	}
	if _, err := sh(remote, checkoutScript(remoteRepo, *ws.Git)); err != nil {
		return errors.Join(err, dropRefs(remote, remoteRepo))
	}
	return transfer(local, ws.Root, Archive{Paths: ws.Secrets}, remote, remoteRepo)
}

func pullGit(remote *Remote, local *Local, ws Workspace, localRepo string) error {
	g := ws.Git
	mark := "cd " + quote(ws.Root) + " && git update-ref " + outPrefix + "handoff " + g.Head +
		" && git update-ref " + outPrefix + "worktree " + g.Worktree + " && git update-ref " + outPrefix + "index " + g.Index
	if _, err := sh(remote, mark); err != nil {
		return err
	}
	refspecs := []string{"+" + outPrefix + "handoff:" + handoffRef, "+" + outPrefix + "worktree:" + worktreeRef, "+" + outPrefix + "index:" + indexRef}
	fetch := "cd " + quote(localRepo) + " && GIT_SSH_COMMAND=" + quote(remote.sshCommand()) +
		" git fetch --quiet --no-tags " + quote(remote.host+":"+ws.Root) + " " + quoteAll(refspecs)
	_, fetchErr := sh(local, fetch)
	if _, err := sh(remote, "cd "+quote(ws.Root)+" && for r in handoff worktree index; do git update-ref -d "+outPrefix+"$r; done"); err != nil {
		return errors.Join(fetchErr, err)
	}
	if fetchErr != nil {
		return fetchErr
	}
	if _, err := sh(local, checkoutScript(localRepo, *g)); err != nil {
		return errors.Join(err, dropRefs(local, localRepo))
	}
	return transfer(remote, ws.Root, Archive{Paths: ws.Secrets}, local, localRepo)
}

func dropRefs(m Machine, repo string) error {
	_, err := sh(m, "cd "+quote(repo)+" && for r in "+handoffRef+" "+worktreeRef+" "+indexRef+"; do git update-ref -d $r; done")
	return err
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
