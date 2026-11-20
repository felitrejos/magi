package main

import (
	"errors"
	"fmt"
	"net/url"
	"regexp"
	"strconv"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

var commitPattern = regexp.MustCompile(`^[0-9a-f]{40}([0-9a-f]{24})?$`)

type Workspace struct {
	Root       string
	MainRoot   string
	Name       string
	Subdir     string
	Git        *GitState
	Secrets    []string
	Untracked  []string `json:"-"`
	LeftOut    []string `json:"-"`
	Size       int64    `json:"-"`
	Changed    int
	LFS        []string
	LFSStore   string
	LFSObjects []string
	Submodules []Workspace
	Subpath    string
	Exclude    []string
}

type GitState struct {
	Origin     string
	OriginKey  string
	RootCommit string
	Branch     string
	Head       string
	Worktree   string
	Index      string
}

const (
	handoffRef  = "refs/magi/handoff"
	worktreeRef = "refs/magi/worktree"
	indexRef    = "refs/magi/index"
	parkedRef   = "refs/worktree/magi/parked"
	outPrefix   = "refs/magi/out/"
)

func readWorkspace(m sh.Machine, cwd string, secretPatterns, exclude []string) (Workspace, error) {
	return inspect(m, cwd, secretPatterns, exclude, true)
}

func inspect(m sh.Machine, cwd string, secretPatterns, exclude []string, outermost bool) (Workspace, error) {
	climb := "0"
	if outermost {
		climb = "1"
	}
	out, err := sh.Run(m, inspectScript, append([]string{cwd, climb}, exclude...)...)
	switch sh.ExitCode(err) {
	case sh.ExitConflicts:
		return Workspace{}, errors.New("the repository has unresolved merge conflicts; resolve them before handing off")
	case sh.ExitInProgress:
		return Workspace{}, errors.New("a merge, rebase or cherry-pick is in progress; finish or abort it before handing off")
	}
	if err != nil {
		return Workspace{}, err
	}
	fields := strings.Split(string(out), "\x00")
	ws := Workspace{Exclude: exclude}
	var real string
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
		case "lfsstore":
			ws.LFSStore = value
		case "changed":
			if ws.Changed, err = strconv.Atoi(value); err != nil {
				return Workspace{}, fmt.Errorf("unexpected changed count from %s", m.Name())
			}
		case "size":
			if ws.Size, err = strconv.ParseInt(value, 10, 64); err != nil {
				return Workspace{}, fmt.Errorf("unexpected size from %s", m.Name())
			}
		case "attrs", "untracked", "leftout", "lfsobjects":
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
			case "untracked":
				ws.Untracked = items
			case "leftout":
				ws.LeftOut = items
			case "lfsobjects":
				ws.LFSObjects = items
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
	if g.Origin != "" {
		aliases, err := resolveAliases(m, []string{g.Origin})
		if err != nil {
			return Workspace{}, err
		}
		g.OriginKey = originKey(g.Origin, aliases)
	}
	ws.Git = g
	ws.Name = baseName(ws.MainRoot)
	ws.Subdir = strings.TrimPrefix(strings.TrimPrefix(real, ws.Root), "/")
	if ws.Subdir == "" {
		ws.Subdir = "."
	}
	submodules, err := gitlinks(m, ws.Root)
	if err != nil {
		return Workspace{}, err
	}
	if ws.Secrets, err = findSecrets(m, ws.Root, secretPatterns, exclude, submodules); err != nil {
		return Workspace{}, err
	}
	for _, path := range submodules {
		sub, err := inspect(m, ws.Root+"/"+path, secretPatterns, exclude, false)
		if err != nil {
			return Workspace{}, fmt.Errorf("submodule %s: %w", path, err)
		}
		sub.Subpath = path
		ws.Submodules = append(ws.Submodules, sub)
	}
	return ws, nil
}

func gitlinks(m sh.Machine, repo string) ([]string, error) {
	out, err := sh.Run(m, gitlinksScript, repo)
	return splitNull(out), err
}

func findSecrets(m sh.Machine, root string, patterns, exclude, submodules []string) ([]string, error) {
	if len(patterns) == 0 {
		return nil, nil
	}
	args := []string{root, ".", "(", "-name", ".git"}
	for _, e := range exclude {
		args = append(args, "-o", "-name", e)
	}
	for _, s := range submodules {
		args = append(args, "-o", "-path", "./"+s)
	}
	args = append(args, ")", "-prune", "-o", "(", "-type", "f", "-o", "-type", "l", "-exec", "test", "-e", "{}", ";", ")", "(")
	for i, p := range patterns {
		if i > 0 {
			args = append(args, "-o")
		}
		if strings.Contains(p, "/") {
			args = append(args, "-path", "*/"+p)
		} else {
			args = append(args, "-name", p)
		}
	}
	args = append(args, ")", "-print0")
	out, err := sh.Run(m, secretsScript, args...)
	if err != nil {
		return nil, err
	}
	var secrets []string
	for _, p := range splitNull(out) {
		secrets = append(secrets, strings.TrimPrefix(p, "./"))
	}
	return secrets, nil
}

func baseName(path string) string {
	return path[strings.LastIndex(path, "/")+1:]
}

func park(m sh.Machine, repo string, ws Workspace) error {
	if _, err := sh.Run(m, `cd -- "$1" && git update-ref "$2" "$3"`, repo, parkedRef, ws.Git.Worktree); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		if err := park(m, repo+"/"+sub.Subpath, sub); err != nil {
			return err
		}
	}
	return nil
}

func checkoutAccepts(m sh.Machine, repo string, ws Workspace) error {
	if _, err := sh.Run(m, checkoutScript, append([]string{"check", repo, "", ws.Git.Worktree, "", ""}, ws.Exclude...)...); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		if err := checkoutAccepts(m, repo+"/"+sub.Subpath, sub); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
	}
	return nil
}

func snapshotRefspecs(g GitState, from func(name, value string) string) []string {
	specs := []string{from(worktreeRef, g.Worktree), from(indexRef, g.Index)}
	if g.Head != "" {
		specs = append(specs, from(handoffRef, g.Head))
	}
	return specs
}

func ensureSubmodule(m sh.Machine, repo string, sub Workspace) error {
	out, err := sh.Run(m, submoduleScript, repo, sub.Subpath, sub.Git.Origin)
	if err != nil || len(out) == 0 {
		return err
	}
	for _, entry := range splitNull(out) {
		if _, path, _ := strings.Cut(entry, "\n"); path == sub.Subpath {
			_, err := sh.Run(m, `cd -- "$1" && git submodule init --quiet -- "$2"`, repo, sub.Subpath)
			return err
		}
	}
	return nil
}

func seedLFS(from, to sh.Machine, ws Workspace, repo string) error {
	if len(ws.LFSObjects) == 0 {
		return nil
	}
	out, err := sh.RunInput(to, lfsScript, []byte(strings.Join(ws.LFSObjects, "\x00")+"\x00"), "missing", repo)
	if err != nil {
		return err
	}
	answer := splitNull(out)
	if len(answer) < 2 {
		return nil
	}
	missing := answer[1:]
	if err := sh.Transfer(from, ws.LFSStore, sh.Archive{Paths: missing}, to, answer[0]); err != nil {
		return err
	}
	_, err = sh.RunInput(to, lfsScript, []byte(strings.Join(missing, "\x00")+"\x00"), "verify", answer[0])
	return err
}

type sendObjects func(ws Workspace, repo string) error

func syncGit(from, to sh.Machine, ws Workspace, repo, mode string, send sendObjects) error {
	if err := send(ws, repo); err != nil {
		return errors.Join(err, dropRefs(to, repo))
	}
	if err := seedLFS(from, to, ws, repo); err != nil {
		return errors.Join(err, dropRefs(to, repo))
	}
	g := ws.Git
	if _, err := sh.Run(to, checkoutScript, append([]string{mode, repo, g.Head, g.Worktree, g.Index, g.Branch}, ws.Exclude...)...); err != nil {
		return errors.Join(err, dropRefs(to, repo))
	}
	if err := finishGit(from, to, ws, repo, mode, send); err != nil {
		return errors.Join(err, park(to, repo, ws))
	}
	return nil
}

func finishGit(from, to sh.Machine, ws Workspace, repo, mode string, send sendObjects) error {
	if ws.LFSStore != "" && ws.Git.Head != "" {
		if _, err := sh.Run(to, `cd -- "$1" && if git lfs version >/dev/null 2>&1 && git config --get filter.lfs.clean >/dev/null; then git lfs checkout >&2; fi`, repo); err != nil {
			return err
		}
	}
	if err := sh.Transfer(from, ws.Root, sh.Archive{Paths: ws.Secrets, Follow: true}, to, repo); err != nil {
		return err
	}
	if err := sh.Transfer(from, ws.Root, sh.Archive{Paths: ws.LFS}, to, repo); err != nil {
		return err
	}
	for _, sub := range ws.Submodules {
		if err := ensureSubmodule(to, repo, sub); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
		if err := syncGit(from, to, sub, repo+"/"+sub.Subpath, mode, send); err != nil {
			return fmt.Errorf("submodule %s: %w", sub.Subpath, err)
		}
	}
	return nil
}

func pushGit(local *sh.Local, remote *sh.Remote, ws Workspace, remoteRepo string) error {
	return sendGit(local, remote, ws, remoteRepo, "apply")
}

func sendGit(local *sh.Local, remote *sh.Remote, ws Workspace, remoteRepo, mode string) error {
	return syncGit(local, remote, ws, remoteRepo, mode, func(ws Workspace, repo string) error {
		refspecs := snapshotRefspecs(*ws.Git, func(ref, id string) string { return id + ":" + ref })
		_, err := sh.Run(local, `dir=$1 to=$3; export GIT_SSH_COMMAND="$2"; shift 3; cd -- "$dir" || exit
if [ "$(git rev-parse --is-shallow-repository)" = true ]; then git push --quiet --force --no-verify --receive-pack='git -c receive.shallowUpdate=true receive-pack' "$to" "$1" || exit; fi
git push --quiet --force --no-verify "$to" "$@"`,
			append([]string{ws.Root, remote.SSHCommand(), remote.Name() + ":" + repo}, refspecs...)...)
		return err
	})
}

func pullGit(remote *sh.Remote, local *sh.Local, ws Workspace, localRepo string) error {
	return syncGit(remote, local, ws, localRepo, "apply", func(ws Workspace, repo string) error {
		mark := []string{ws.Root}
		for _, spec := range snapshotRefspecs(*ws.Git, func(ref, id string) string { return outPrefix + baseName(ref) + " " + id }) {
			name, id, _ := strings.Cut(spec, " ")
			mark = append(mark, name, id)
		}
		if _, err := sh.Run(remote, `cd -- "$1" && shift && while [ $# -gt 0 ]; do git update-ref "$1" "$2" || exit; shift 2; done`, mark...); err != nil {
			return err
		}
		refspecs := snapshotRefspecs(*ws.Git, func(ref, _ string) string { return "+" + outPrefix + baseName(ref) + ":" + ref })
		_, fetchErr := sh.Run(local, `dir=$1 ssh=$2; shift 2; cd -- "$dir" && GIT_SSH_COMMAND=$ssh git fetch --quiet --atomic --update-shallow --no-tags "$@"`,
			append([]string{repo, remote.SSHCommand(), remote.Name() + ":" + ws.Root}, refspecs...)...)
		cleanErr := deleteRefs(remote, ws.Root, outPrefix+"handoff", outPrefix+"worktree", outPrefix+"index")
		return errors.Join(fetchErr, cleanErr)
	})
}

func dropRefs(m sh.Machine, repo string) error {
	return deleteRefs(m, repo, handoffRef, worktreeRef, indexRef)
}

func deleteRefs(m sh.Machine, repo string, refs ...string) error {
	_, err := sh.Run(m, `cd -- "$1" && shift && for r; do git update-ref -d "$r"; done`, append([]string{repo}, refs...)...)
	return err
}

func sshStyle(raw string) bool {
	return strings.HasPrefix(raw, "ssh://") || !strings.Contains(raw, "://") && strings.Contains(raw, ":")
}

func resolveAliases(m sh.Machine, origins []string) (map[string]string, error) {
	var hosts []string
	seen := map[string]bool{}
	for _, o := range origins {
		host, _, _ := strings.Cut(normalizeRemote(o), "/")
		if sshStyle(o) && !seen[host] {
			seen[host] = true
			hosts = append(hosts, host)
		}
	}
	aliases := map[string]string{}
	if len(hosts) == 0 {
		return aliases, nil
	}
	out, err := sh.Run(m, hostsScript, hosts...)
	if err != nil {
		return nil, err
	}
	for i, name := range strings.Split(string(out), "\x00") {
		if i < len(hosts) && name != "" {
			aliases[hosts[i]] = strings.ToLower(name)
		}
	}
	return aliases, nil
}

func originKey(raw string, aliases map[string]string) string {
	host, path, _ := strings.Cut(normalizeRemote(raw), "/")
	if name := aliases[host]; name != "" {
		host = name
	}
	return host + "/" + path
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

func unparkRepo(m sh.Machine, repo string) error {
	if err := deleteRefs(m, repo, parkedRef); err != nil {
		return err
	}
	links, err := gitlinks(m, repo)
	if err != nil {
		return err
	}
	for _, link := range links {
		if err := unparkRepo(m, repo+"/"+link); err != nil {
			return err
		}
	}
	return nil
}
