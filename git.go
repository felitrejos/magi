package main

import (
	"errors"
	"fmt"
	"net/url"
	"regexp"
	"strconv"
	"strings"
)

var commitPattern = regexp.MustCompile(`^[0-9a-f]{40}([0-9a-f]{24})?$`)

type Workspace struct {
	Root       string
	MainRoot   string
	Name       string
	Subdir     string
	Git        *GitState
	Secrets    []string
	LFS        []string
	Submodules []Workspace
	Subpath    string
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
	exitConflicts  = 8
	exitInProgress = 9
)

func inspectScript(cwd string, secretPatterns []string) string {
	var pathspecs []string
	for _, p := range secretPatterns {
		pathspecs = append(pathspecs, quote(":(glob)**/"+p))
	}
	secrets := ""
	if len(pathspecs) > 0 {
		secrets = `git ls-files -z --others --ignored --exclude-standard --directory -- ` + strings.Join(pathspecs, " ") + ` > "$d/secrets"; list secrets "$d/secrets"; `
	}
	return `set -e; export LC_ALL=C; cd -- ` + quote(cwd) + `; emit() { printf '%s\0%s\0' "$1" "$2"; }; ` +
		`list() { emit "$1" "$(tr -cd '\0' < "$2" | wc -c | tr -d ' ')"; cat "$2"; }; emit cwd "$(pwd -P)"; ` +
		`if ! top=$(git rev-parse --show-toplevel 2>&1); then case "$top" in *"not a git repository"*) emit plain 1; exit 0;; esac; echo "$top" >&2; exit 1; fi; ` +
		`emit root "$top"; cd "$top"; d=$(mktemp -d); trap 'rm -rf "$d"' EXIT; ` +
		`common=$(git rev-parse --path-format=absolute --git-common-dir); emit mainroot "$(cd "$(dirname "$common")" && pwd -P)"; ` +
		`emit origin "$(git remote get-url origin 2>/dev/null || :)"; branch=$(git symbolic-ref --short -q HEAD || :); emit branch "$branch"; ` +
		`head=; parent=; if git rev-parse -q --verify HEAD >/dev/null; then roots=$(git rev-list --max-parents=0 HEAD); emit rootcommit "$(printf '%s\n' $roots | sort | head -n 1)"; ` +
		`head=$(git rev-parse HEAD); emit head "$head"; parent="-p $head"; fi; ` +
		`[ -z "$(git ls-files --unmerged)" ] || exit ` + fmt.Sprint(exitConflicts) + `; ` +
		`for m in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD rebase-merge rebase-apply; do [ ! -e "$(git rev-parse --path-format=absolute --git-path $m)" ] || exit ` + fmt.Sprint(exitInProgress) + `; done; ` +
		`itree=$(git write-tree); x=$(git rev-parse --path-format=absolute --git-path index); if [ -f "$x" ]; then cp "$x" "$d/index"; fi; ` +
		`GIT_INDEX_FILE="$d/index" git add -A; wtree=$(GIT_INDEX_FILE="$d/index" git write-tree); ` +
		`export GIT_AUTHOR_NAME=cielago GIT_AUTHOR_EMAIL=cielago@localhost GIT_COMMITTER_NAME=cielago GIT_COMMITTER_EMAIL=cielago@localhost; ` +
		`wt=$(git commit-tree "$wtree" $parent -m 'cielago: working tree'); idx=$(git commit-tree "$itree" $parent -m 'cielago: staging area'); emit worktree "$wt"; emit index "$idx"; ` +
		`base=${head:-$(git hash-object -t tree /dev/null)}; ` +
		`git diff -z --name-only --no-renames --diff-filter=d "$base" "$wt" > "$d/changed"; git check-attr -z --stdin filter < "$d/changed" > "$d/attrs"; list attrs "$d/attrs"; ` +
		`git submodule --quiet foreach 'printf "%s\0" "$sm_path"' > "$d/submodules"; list submodules "$d/submodules"; ` +
		secrets
}

func readWorkspace(m Machine, cwd string, secretPatterns []string) (Workspace, error) {
	out, err := sh(m, inspectScript(cwd, secretPatterns))
	switch exitCode(err) {
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
	var submodules []string
	g := &GitState{}
	for i := 0; i+1 < len(fields); i += 2 {
		key, value := fields[i], fields[i+1]
		switch key {
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
		case "attrs", "submodules", "secrets":
			n, err := strconv.Atoi(value)
			if err != nil || i+2+n > len(fields) {
				return Workspace{}, fmt.Errorf("unexpected %s list from %s", key, m.Name())
			}
			items := fields[i+2 : i+2+n]
			i += n
			switch key {
			case "attrs":
				for j := 0; j+2 < len(items); j += 3 {
					if items[j+1] == "filter" && items[j+2] == "lfs" {
						ws.LFS = append(ws.LFS, items[j])
					}
				}
			case "submodules":
				submodules = items
			case "secrets":
				for _, s := range items {
					if !strings.HasSuffix(s, "/") {
						ws.Secrets = append(ws.Secrets, s)
					}
				}
			}
		}
	}
	ids := []string{g.Worktree, g.Index}
	if g.Head != "" {
		ids = append(ids, g.Head)
	}
	for _, id := range ids {
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
	for _, path := range submodules {
		sub, err := readWorkspace(m, ws.Root+"/"+path, secretPatterns)
		if err != nil {
			return Workspace{}, fmt.Errorf("submodule %s: %w", path, err)
		}
		sub.Subpath = path
		ws.Submodules = append(ws.Submodules, sub)
	}
	return ws, nil
}

func baseName(path string) string {
	return path[strings.LastIndex(path, "/")+1:]
}

func park(m Machine, repo string, ws Workspace) error {
	if _, err := sh(m, "cd "+quote(repo)+" && git update-ref "+parkedRef+" "+quote(ws.Git.Worktree)); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		if err := park(m, repo+"/"+sub.Subpath, sub); err != nil {
			return err
		}
	}
	return nil
}

const dirtyCheck = `tree_now() { d=$(mktemp -d); x=$(git rev-parse --path-format=absolute --git-path index); if [ -f "$x" ]; then cp "$x" "$d/index"; fi; ` +
	`GIT_INDEX_FILE="$d/index" git add -A >/dev/null; GIT_INDEX_FILE="$d/index" git write-tree; rm -rf "$d"; }; ` +
	`force=; if [ -n "$(git status --porcelain --untracked-files=all --ignore-submodules=none)" ]; then ` +
	`now=$(tree_now); parked=$(git rev-parse -q --verify '` + parkedRef + `^{tree}' || :); ` +
	`if [ "$now" = "$parked" ] || [ "$now" = "$(git rev-parse -q --verify "$W^{tree}" || :)" ]; then force=1; else ` +
	`echo "$PWD has uncommitted changes made on this machine; commit or stash them first" >&2; exit 3; fi; fi; `

func checkoutAccepts(m Machine, repo string, ws Workspace) error {
	if _, err := sh(m, "set -e; cd "+quote(repo)+"; [ -e .git ] || exit 0; W="+quote(ws.Git.Worktree)+"; "+dirtyCheck); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		if err := checkoutAccepts(m, repo+"/"+sub.Subpath, sub); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
	}
	return nil
}

func checkoutScript(repo string, g GitState) string {
	var s strings.Builder
	s.WriteString("set -e; cd " + quote(repo) + "; H=" + quote(g.Head) + "; W=" + quote(g.Worktree) + "; I=" + quote(g.Index) + "; ")
	s.WriteString(`base=${H:-$(git hash-object -t tree /dev/null)}; `)
	s.WriteString(dirtyCheck)
	s.WriteString(`git -c core.quotepath=false diff --name-only --diff-filter=A "$base" "$W" | while IFS= read -r p; do ` +
		`if [ -e "$p" ] && git check-ignore -q -- "$p" && [ "$(git hash-object -- "$p")" != "$(git rev-parse "$W:$p")" ]; then ` +
		`printf '%s exists here, is ignored, and would be overwritten; move it away first\n' "$p" >&2; exit 6; fi; done; `)
	branchRef := quote("refs/heads/" + g.Branch)
	switch {
	case g.Head == "":
		s.WriteString("if git rev-parse --verify --quiet " + branchRef + " >/dev/null; then " +
			"printf 'branch %s has commits this machine does not; pull them here first\\n' " + quote(g.Branch) + " >&2; exit 4; fi; ")
		s.WriteString(`git symbolic-ref HEAD ` + branchRef + `; if [ -n "$force" ]; then git clean -fdq; fi; GIT_LFS_SKIP_SMUDGE=1 git read-tree --reset -u "$W"; `)
	case g.Branch == "":
		s.WriteString(`git checkout --quiet ${force:+--force} --detach "$H"; if [ -n "$force" ]; then git clean -fdq; fi; GIT_LFS_SKIP_SMUDGE=1 git read-tree -m -u HEAD "$W"; `)
	default:
		s.WriteString("if git rev-parse --verify --quiet " + branchRef + " >/dev/null; then " +
			`rc=0; git merge-base --is-ancestor ` + branchRef + ` "$H" || rc=$?; ` +
			"if [ $rc -eq 1 ]; then printf 'branch %s has commits this machine does not; pull them here first\\n' " + quote(g.Branch) + " >&2; exit 4; fi; " +
			"[ $rc -eq 0 ]; fi; ")
		s.WriteString(`git checkout --quiet ${force:+--force} -B ` + quote(g.Branch) + ` "$H"; if [ -n "$force" ]; then git clean -fdq; fi; GIT_LFS_SKIP_SMUDGE=1 git read-tree -m -u HEAD "$W"; `)
	}
	s.WriteString(`git read-tree "$I"; `)
	s.WriteString("for r in " + handoffRef + " " + worktreeRef + " " + indexRef + " " + parkedRef + "; do git update-ref -d $r; done")
	return s.String()
}

func snapshotRefspecs(g GitState, from func(name, value string) string) []string {
	specs := []string{from(worktreeRef, g.Worktree), from(indexRef, g.Index)}
	if g.Head != "" {
		specs = append(specs, from(handoffRef, g.Head))
	}
	return specs
}

func ensureSubmodule(m Machine, repo string, sub Workspace) error {
	dir := quote(repo + "/" + sub.Subpath)
	script := "set -e; [ ! -e " + dir + "/.git ] || exit 0; git init --quiet " + dir + "; "
	if sub.Git.Origin != "" {
		script += "git -C " + dir + " remote add origin " + quote(sub.Git.Origin) + "; "
	}
	_, err := sh(m, script+"git -C "+quote(repo)+" submodule init --quiet -- "+quote(sub.Subpath))
	return err
}

func pushGit(local *Local, remote *Remote, ws Workspace, remoteRepo string) error {
	refspecs := snapshotRefspecs(*ws.Git, func(ref, id string) string { return id + ":" + ref })
	push := "cd " + quote(ws.Root) + " && GIT_SSH_COMMAND=" + quote(remote.sshCommand()) +
		" git push --quiet --force --no-verify " + quote(remote.host+":"+remoteRepo) + " " + quoteAll(refspecs)
	if _, err := sh(local, push); err != nil {
		return err
	}
	if _, err := sh(remote, checkoutScript(remoteRepo, *ws.Git)); err != nil {
		return errors.Join(err, dropRefs(remote, remoteRepo))
	}
	if err := transfer(local, ws.Root, Archive{Paths: append(append([]string{}, ws.Secrets...), ws.LFS...)}, remote, remoteRepo); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		path := remoteRepo + "/" + sub.Subpath
		if err := ensureSubmodule(remote, remoteRepo, sub); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
		if err := pushGit(local, remote, sub, path); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
	}
	return nil
}

func pullGit(remote *Remote, local *Local, ws Workspace, localRepo string) error {
	g := *ws.Git
	var mark []string
	for _, spec := range snapshotRefspecs(g, func(ref, id string) string { return outPrefix + baseName(ref) + " " + id }) {
		mark = append(mark, "git update-ref "+spec)
	}
	if _, err := sh(remote, "cd "+quote(ws.Root)+" && "+strings.Join(mark, " && ")); err != nil {
		return err
	}
	refspecs := snapshotRefspecs(g, func(ref, _ string) string { return "+" + outPrefix + baseName(ref) + ":" + ref })
	fetch := "cd " + quote(localRepo) + " && GIT_SSH_COMMAND=" + quote(remote.sshCommand()) +
		" git fetch --quiet --no-tags " + quote(remote.host+":"+ws.Root) + " " + quoteAll(refspecs)
	_, fetchErr := sh(local, fetch)
	if _, err := sh(remote, "cd "+quote(ws.Root)+" && for r in handoff worktree index; do git update-ref -d "+outPrefix+"$r; done"); err != nil {
		return errors.Join(fetchErr, err)
	}
	if fetchErr != nil {
		return fetchErr
	}
	if _, err := sh(local, checkoutScript(localRepo, g)); err != nil {
		return errors.Join(err, dropRefs(local, localRepo))
	}
	if err := transfer(remote, ws.Root, Archive{Paths: append(append([]string{}, ws.Secrets...), ws.LFS...)}, local, localRepo); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		path := localRepo + "/" + sub.Subpath
		if err := ensureSubmodule(local, localRepo, sub); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
		if err := pullGit(remote, local, sub, path); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
	}
	return nil
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
