package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

const fakeSSH = `#!/bin/sh
host=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    --) shift; break ;;
    *) if [ -z "$host" ]; then host=$1; shift; else break; fi ;;
  esac
done
exec sh -c "$*"
`

type fixture struct {
	t      *testing.T
	home   string
	local  string
	remote string
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(home, "bin")
	f := &fixture{t: t, home: home, local: filepath.Join(home, "laptop", "acme", "api"), remote: filepath.Join(home, "developer", "acme", "api")}
	f.write(bin, "ssh", fakeSSH)
	if err := os.Chmod(filepath.Join(bin, "ssh"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("PATH", bin+":"+os.Getenv("PATH"))
	t.Setenv("GIT_CONFIG_GLOBAL", "/dev/null")
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_AUTHOR_NAME", "t")
	t.Setenv("GIT_AUTHOR_EMAIL", "t@t")
	t.Setenv("GIT_COMMITTER_NAME", "t")
	t.Setenv("GIT_COMMITTER_EMAIL", "t@t")

	f.git(home, "init", "-q", "-b", "main", f.local)
	f.git(f.local, "remote", "add", "origin", "git@github.com:acme/api.git")
	f.write(f.local, ".gitignore", ".env*\nnode_modules/\n")
	f.write(f.local, "kept.txt", "v1")
	f.write(f.local, "edited.txt", "v1")
	f.write(f.local, "gone.txt", "v1")
	f.write(f.local, "staged-gone.txt", "v1")
	f.git(f.local, "add", ".")
	f.git(f.local, "commit", "-q", "-m", "init")
	f.git(home, "clone", "-q", f.local, f.remote)
	f.git(f.remote, "remote", "set-url", "origin", "https://github.com/acme/api")
	return f
}

func (f *fixture) git(dir string, args ...string) string {
	f.t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		f.t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func (f *fixture) write(dir, name, body string) {
	f.t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) read(dir, name string) string {
	f.t.Helper()
	data, err := os.ReadFile(filepath.Join(dir, name))
	if err != nil {
		return "<missing>"
	}
	return string(data)
}

func (f *fixture) remoteHost() *Remote {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	r, err := newRemote(ctx, "fakehost")
	if err != nil {
		f.t.Fatal(err)
	}
	return r
}

func (f *fixture) machine() *Local {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	l, err := newLocal(ctx)
	if err != nil {
		f.t.Fatal(err)
	}
	return l
}

func (f *fixture) workspace(dir string) Workspace {
	f.t.Helper()
	ws, err := readWorkspace(f.machine(), dir, []string{".env", ".env.*"})
	if err != nil {
		f.t.Fatal(err)
	}
	return ws
}

func (f *fixture) messUp(dir string) {
	f.t.Helper()
	f.write(dir, "committed.txt", "from a commit")
	f.git(dir, "add", "committed.txt")
	f.git(dir, "commit", "-q", "-m", "work")
	f.write(dir, "edited.txt", "staged")
	f.git(dir, "add", "edited.txt")
	f.write(dir, "edited.txt", "staged then edited")
	f.write(dir, "new-staged.txt", "new")
	f.git(dir, "add", "new-staged.txt")
	f.write(dir, "dir with space/untracked.bin", "\x00\x01binary")
	if err := os.Remove(filepath.Join(dir, "gone.txt")); err != nil {
		f.t.Fatal(err)
	}
	f.git(dir, "rm", "-q", "staged-gone.txt")
	f.write(dir, ".env", "SECRET=1")
	f.write(dir, "node_modules/pkg/.env", "not ours")
}

func TestHandoffReproducesCommitsStagingAndTheWorkingTree(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	ws := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), ws, f.remote); err != nil {
		t.Fatal(err)
	}
	if got, want := f.git(f.remote, "rev-parse", "HEAD"), f.git(f.local, "rev-parse", "HEAD"); got != want {
		t.Errorf("HEAD = %s, want %s", got, want)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("status on the host:\n%s\nwant:\n%s", got, want)
	}
	for _, name := range []string{"edited.txt", "new-staged.txt", "dir with space/untracked.bin", ".env", "gone.txt"} {
		if got, want := f.read(f.remote, name), f.read(f.local, name); got != want {
			t.Errorf("%s = %q, want %q", name, got, want)
		}
	}
	if got := f.read(f.remote, "node_modules/pkg/.env"); got != "<missing>" {
		t.Errorf("a .env inside node_modules was copied: %q", got)
	}
}

func TestHandoffRefusesAHostWithItsOwnUncommittedWork(t *testing.T) {
	f := newFixture(t)
	f.write(f.remote, "edited.txt", "host-only edit")
	err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote)
	if err == nil || !strings.Contains(err.Error(), "uncommitted changes") {
		t.Fatalf("err = %v, want an uncommitted changes refusal", err)
	}
	if got := f.read(f.remote, "edited.txt"); got != "host-only edit" {
		t.Errorf("host edit was overwritten: %q", got)
	}
}

func TestSessionCanComeBackToTheCheckoutItLeft(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), out, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out); err != nil {
		t.Fatal(err)
	}

	f.write(f.remote, "edited.txt", "continued on the host")
	f.write(f.remote, "host-new.txt", "made on the host")
	back := f.workspace(f.remote)
	if err := pushGit(f.machine(), f.remoteHost(), back, f.local); err != nil {
		t.Fatalf("handing back to the parked laptop failed: %v", err)
	}
	if got, want := f.git(f.local, "status", "--porcelain"), f.git(f.remote, "status", "--porcelain"); got != want {
		t.Errorf("laptop status after the round trip:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.local, "edited.txt") + "|" + f.read(f.local, "host-new.txt"); got != "continued on the host|made on the host" {
		t.Errorf("laptop files after the round trip = %q", got)
	}
	if out, _ := exec.Command("git", "-C", f.local, "rev-parse", "-q", "--verify", parkedRef).Output(); len(out) != 0 {
		t.Error("the laptop is still marked as parked after the session came back")
	}
}

func TestSessionCannotComeBackOverWorkDoneAfterLeaving(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), out, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out); err != nil {
		t.Fatal(err)
	}
	f.write(f.local, "kept.txt", "edited on the laptop after the handoff")

	err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.remote), f.local)
	if err == nil || !strings.Contains(err.Error(), "uncommitted changes") {
		t.Fatalf("err = %v, want a refusal", err)
	}
	if got := f.read(f.local, "kept.txt"); got != "edited on the laptop after the handoff" {
		t.Errorf("laptop edit was overwritten: %q", got)
	}
}

func TestARetryAfterAFailedHandoffIsAccepted(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	ws := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), ws, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote); err != nil {
		t.Fatalf("a second attempt with the same changes was refused: %v", err)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("status after the retry:\n%s\nwant:\n%s", got, want)
	}
}

func TestHiddenUntrackedFilesOnTheHostAreNotOverwritten(t *testing.T) {
	f := newFixture(t)
	f.git(f.remote, "config", "status.showUntrackedFiles", "no")
	f.write(f.remote, "committed.txt", "HOST ONLY DATA")
	f.messUp(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote); err == nil {
		t.Fatal("handoff went ahead over an untracked file that status hides")
	}
	if got := f.read(f.remote, "committed.txt"); got != "HOST ONLY DATA" {
		t.Errorf("host-only file was overwritten: %q", got)
	}
}

func TestIgnoredFilesOnTheHostAreNotOverwritten(t *testing.T) {
	f := newFixture(t)
	f.write(f.remote, ".git/info/exclude", "scratch.md\n")
	f.write(f.remote, "scratch.md", "host notes")
	f.write(f.local, "scratch.md", "laptop notes")
	err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote)
	if err == nil || !strings.Contains(err.Error(), "would be overwritten") {
		t.Fatalf("err = %v, want a refusal", err)
	}
	if got := f.read(f.remote, "scratch.md"); got != "host notes" {
		t.Errorf("ignored host file was overwritten: %q", got)
	}
}

func TestAFileAddedOnTheParkedMachineIsNotCleanedAway(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), out, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out); err != nil {
		t.Fatal(err)
	}
	f.write(f.local, "written-after.txt", "keep me")
	if err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.remote), f.local); err == nil {
		t.Fatal("handoff back went ahead although the laptop gained a file after parking")
	}
	if got := f.read(f.local, "written-after.txt"); got != "keep me" {
		t.Errorf("new laptop file was removed: %q", got)
	}
}

func TestReadWorkspaceRefusesAMergeInProgress(t *testing.T) {
	f := newFixture(t)
	f.git(f.local, "checkout", "-q", "-b", "side")
	f.write(f.local, "side.txt", "side")
	f.git(f.local, "add", "side.txt")
	f.git(f.local, "commit", "-q", "-m", "side")
	f.git(f.local, "checkout", "-q", "main")
	f.git(f.local, "merge", "--no-commit", "--no-ff", "-q", "side")
	if _, err := readWorkspace(f.machine(), f.local, nil); err == nil || !strings.Contains(err.Error(), "in progress") {
		t.Fatalf("err = %v, want a merge-in-progress refusal", err)
	}
}

func TestReadWorkspaceHandlesACheckoutWithoutAnIndex(t *testing.T) {
	f := newFixture(t)
	bare := filepath.Join(f.home, "laptop", "no-index")
	f.git(f.home, "clone", "-q", "--no-checkout", f.local, bare)
	ws := f.workspace(bare)
	if ws.Git == nil || ws.Git.Worktree == "" {
		t.Fatalf("workspace = %+v, want a snapshot", ws)
	}
}

func TestPlainFolderStateTellsNewOwnAndForeignFoldersApart(t *testing.T) {
	f := newFixture(t)
	r := f.remoteHost()
	ours := filepath.Join(f.home, "developer", "notes", "notes")
	foreign := filepath.Join(f.home, "developer", "other", "notes")
	f.write(ours, "todo.md", "x")
	f.write(foreign, "todo.md", "y")
	if err := registerFolder(r, ours); err != nil {
		t.Fatal(err)
	}
	if err := registerFolder(r, ours); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]string{
		ours:                                 "ours",
		foreign:                              "foreign",
		f.remote:                             "git",
		filepath.Join(f.home, "nope", "new"): "new",
	} {
		if got, err := folderState(r, path); err != nil || got != want {
			t.Errorf("folderState(%s) = %q, %v; want %q", path, got, err, want)
		}
	}
	if got := f.read(f.home, ".cielago/folders"); got != ours+"\n" {
		t.Errorf("registry = %q, want one entry", got)
	}
}

func TestHandoffRefusesADivergedBranchWithoutRunningItsName(t *testing.T) {
	f := newFixture(t)
	branch := `x";touch${IFS}PWNED;"y`
	f.git(f.local, "check-ref-format", "--branch", branch)
	f.git(f.local, "checkout", "-q", "-b", branch)
	f.git(f.remote, "checkout", "-q", "-b", branch)
	f.write(f.remote, "host.txt", "host commit")
	f.git(f.remote, "add", "host.txt")
	f.git(f.remote, "commit", "-q", "-m", "host only")

	err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote)
	if err == nil || !strings.Contains(err.Error(), "has commits this machine does not") {
		t.Fatalf("err = %v, want divergence refusal", err)
	}
	for _, dir := range []string{f.remote, f.local, f.home} {
		if _, err := os.Stat(filepath.Join(dir, "PWNED")); err == nil {
			t.Fatalf("branch name was executed as shell code (PWNED in %s)", dir)
		}
	}
}

func TestHandoffCreatesACloneForAProjectTheHostDoesNotHave(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	r := f.remoteHost()
	ws := f.workspace(f.local)
	clones, err := listClones(r, []string{"~/developer"})
	if err != nil {
		t.Fatal(err)
	}
	ws.Git.Origin = "git@github.com:acme/brand-new.git"
	if repo, err := matchClone(clones, ws, r.host); err != nil || repo != "" {
		t.Fatalf("matchClone = %q, %v; want no clone", repo, err)
	}
	dir := filepath.Join(f.home, "developer", "acme", "brand-new")
	if err := createClone(r, dir, ws.Git.Origin); err != nil {
		t.Fatal(err)
	}
	if err := pushGit(f.machine(), r, ws, dir); err != nil {
		t.Fatal(err)
	}
	if got, want := f.git(dir, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("new clone status:\n%s\nwant:\n%s", got, want)
	}
	if got := f.git(dir, "branch", "--show-current"); got != "main" {
		t.Errorf("new clone branch = %q, want main", got)
	}
}

func TestMatchCloneFindsARepoWithNoRemoteByItsFirstCommit(t *testing.T) {
	f := newFixture(t)
	f.git(f.local, "remote", "remove", "origin")
	f.git(f.remote, "remote", "remove", "origin")
	r := f.remoteHost()
	clones, err := listClones(r, []string{"~/developer"})
	if err != nil {
		t.Fatal(err)
	}
	repo, err := matchClone(clones, f.workspace(f.local), r.host)
	if err != nil || repo != f.remote {
		t.Fatalf("matchClone = %q, %v; want %q", repo, err, f.remote)
	}
}

func TestPlainFolderIsCopiedWithoutExcludedPaths(t *testing.T) {
	f := newFixture(t)
	notes := filepath.Join(f.home, "laptop", "notes")
	f.write(notes, "todo.md", "ship it")
	f.write(notes, "drafts/idea.md", "idea")
	f.write(notes, "node_modules/big/index.js", "huge")
	ws := f.workspace(notes)
	if ws.Git != nil || ws.Root != notes || ws.Name != "notes" {
		t.Fatalf("workspace = %+v, want a plain folder", ws)
	}
	dest := filepath.Join(f.home, "developer", "notes", "notes")
	if err := transfer(f.machine(), ws.Root, Archive{Paths: []string{"."}, Exclude: []string{"node_modules"}}, f.remoteHost(), dest); err != nil {
		t.Fatal(err)
	}
	if got := f.read(dest, "todo.md") + "|" + f.read(dest, "drafts/idea.md"); got != "ship it|idea" {
		t.Errorf("copied files = %q", got)
	}
	if got := f.read(dest, "node_modules/big/index.js"); got != "<missing>" {
		t.Errorf("an excluded folder was copied: %q", got)
	}
}

func TestMemoryIsMergedNotReplaced(t *testing.T) {
	f := newFixture(t)
	local := filepath.Join(f.home, "laptop-memory")
	remote := filepath.Join(f.home, "host-memory")
	f.write(local, "MEMORY.md", "- [Deploys](deploys.md) — laptop wording\n- [Style](style.md) — tabs\n")
	f.write(local, "style.md", "tabs")
	f.write(local, "deploys.md", "laptop deploys")
	f.write(remote, "MEMORY.md", "- [Deploys](deploys.md) — host wording\n- [Hosts](hosts.md) — names\n")
	f.write(remote, "hosts.md", "names")
	f.write(remote, "deploys.md", "host deploys, newer")
	future := time.Now().Add(time.Hour)
	if err := os.Chtimes(filepath.Join(remote, "deploys.md"), future, future); err != nil {
		t.Fatal(err)
	}

	if err := syncMemory(f.machine(), f.machine(), local, f.remoteHost(), remote); err != nil {
		t.Fatal(err)
	}
	want := "- [Deploys](deploys.md) — laptop wording\n- [Style](style.md) — tabs\n- [Hosts](hosts.md) — names\n"
	if got := f.read(remote, "MEMORY.md"); got != want {
		t.Errorf("MEMORY.md =\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(remote, "style.md") + "|" + f.read(remote, "hosts.md") + "|" + f.read(remote, "deploys.md"); got != "tabs|names|host deploys, newer" {
		t.Errorf("memory files = %q", got)
	}
}

func TestHandoffPromptCarriesTheNoteAndSubagentTranscripts(t *testing.T) {
	p := handoffPrompt(Prompt{From: "laptop", To: "server", OldDir: "/a", NewDir: "/b", Git: true,
		Note: "dev server on :3000; a subagent was halfway through the API tests", Sidecar: "/h/.claude/projects/-b/s1"})
	for _, want := range []string{"dev server on :3000", "/h/.claude/projects/-b/s1/subagents", "staged and unstaged", "confirming you're on server"} {
		if !strings.Contains(p, want) {
			t.Errorf("prompt is missing %q:\n%s", want, p)
		}
	}
}

func TestWaitForRemoteControlIgnoresBrokenAndStaleSessionFiles(t *testing.T) {
	f := newFixture(t)
	launched := time.Now()
	sessions := filepath.Join(f.home, ".claude", "sessions")
	f.write(sessions, "1.json", "{not json")
	f.write(sessions, "2.json", `{"sessionId":"s1","startedAt":1000,"bridgeSessionId":"old"}`)

	ctx, cancel := context.WithTimeout(context.Background(), 1500*time.Millisecond)
	defer cancel()
	r := f.remoteHost()
	r.SetContext(ctx)
	if err := waitForRemoteControl(r, "s1", launched); err == nil {
		t.Fatal("a stale session file from an earlier run counted as the new session")
	}

	f.write(sessions, "3.json", `{"sessionId":"s1","startedAt":`+strconv.FormatInt(launched.UnixMilli(), 10)+`,"bridgeSessionId":"cse_1"}`)
	if err := waitForRemoteControl(f.remoteHost(), "s1", launched); err != nil {
		t.Fatal(err)
	}
}

func TestWaitForRemoteControlWorksWithNoSessionFiles(t *testing.T) {
	f := newFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 1200*time.Millisecond)
	defer cancel()
	r := f.remoteHost()
	r.SetContext(ctx)
	err := waitForRemoteControl(r, "s1", time.Now())
	if err == nil || !strings.Contains(err.Error(), "in time") {
		t.Fatalf("err = %v, want a timeout rather than a script failure", err)
	}
}

func TestIsTrustedAcceptsATrustedParent(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, ".claude.json", `{"projects":{"`+f.home+`/developer":{"hasTrustDialogAccepted":true}}}`)
	r := f.remoteHost()
	if trusted, err := isTrusted(r, f.remote); err != nil || !trusted {
		t.Fatalf("trusted = %v, err = %v; want a trusted parent to count", trusted, err)
	}
	if trusted, err := isTrusted(r, filepath.Join(f.home, "elsewhere")); err != nil || trusted {
		t.Fatalf("trusted = %v, err = %v; want an untrusted folder to report false", trusted, err)
	}
}

func TestStopRemoteSessionStopsTheSessionWithoutKillingItself(t *testing.T) {
	f := newFixture(t)
	sessionID := "11111111-2222-3333-4444-555555555555"
	fake := exec.Command("sh", "-c", "sleep 30; : --resume "+sessionID)
	if err := fake.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- fake.Wait() }()
	if err := stopSession(f.remoteHost(), sessionID); err != nil {
		t.Fatalf("stopRemoteSession failed: %v", err)
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		fake.Process.Kill()
		t.Fatal("the session process was not stopped")
	}
}

func TestReadWorkspaceNamesAWorktreeAfterItsMainCheckout(t *testing.T) {
	f := newFixture(t)
	tree := filepath.Join(f.local, ".claude", "worktrees", "feat")
	f.git(f.local, "worktree", "add", "-q", "-b", "feat", tree)
	ws := f.workspace(tree)
	if ws.Name != "api" || ws.MainRoot != f.local {
		t.Errorf("Name = %q, MainRoot = %q; want api, %q", ws.Name, ws.MainRoot, f.local)
	}
}

func TestReadWorkspaceRefusesARepositoryWithoutCommits(t *testing.T) {
	f := newFixture(t)
	empty := filepath.Join(f.home, "laptop", "empty")
	f.git(f.home, "init", "-q", empty)
	if _, err := readWorkspace(f.machine(), empty, nil); err == nil || !strings.Contains(err.Error(), "first commit") {
		t.Fatalf("err = %v, want a first-commit error", err)
	}
}

func TestHostRejectsRelativeRoots(t *testing.T) {
	h := Host{SSH: "s", Claude: "c", Launcher: "rex", RexServer: "r", Roots: []string{"developer"}}
	if err := h.validate("server"); err == nil {
		t.Fatal("a relative root was accepted")
	}
	h.Roots = []string{"~/developer", "/srv/code"}
	if err := h.validate("server"); err != nil {
		t.Fatal(err)
	}
}

func TestGroupsMirrorTheLaptopAndKeepTheServersLayout(t *testing.T) {
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	for _, dir := range []string{"Projects/acme/api", "Projects/notes", "elsewhere/tool"} {
		if err := os.MkdirAll(filepath.Join(home, dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	roots := []string{"~/Missing", "~/Projects"}
	for dir, want := range map[string]string{
		"Projects/acme/api": "acme",
		"Projects/notes":    "notes",
		"elsewhere/tool":    "tool",
		"Projects":          "Projects",
	} {
		if got, err := localGroup(filepath.Join(home, dir), roots); err != nil || got != want {
			t.Errorf("localGroup(%s) = %q, %v; want %q", dir, got, err, want)
		}
	}
	for clone, want := range map[string]string{
		"/home/me/developer/acme/api-server": "acme",
		"/home/me/developer/notes":           "notes",
		"/srv/other/thing":                   "other",
	} {
		if got := serverGroup(clone, []string{"~/developer"}, "/home/me"); got != want {
			t.Errorf("serverGroup(%s) = %q, want %q", clone, got, want)
		}
	}
}

func TestNormalizeRemoteMatchesSshAndHttpsForms(t *testing.T) {
	want := "github.com/acme/api"
	for _, raw := range []string{
		"https://github.com/acme/api.git",
		"https://GitHub.com/acme/api/",
		"git@github.com:acme/api.git",
		"ssh://git@github.com/acme/api",
	} {
		if got := normalizeRemote(raw); got != want {
			t.Errorf("normalizeRemote(%q) = %q, want %q", raw, got, want)
		}
	}
}

func TestProjectSlugMatchesClaudeFolderNames(t *testing.T) {
	for dir, want := range map[string]string{
		"/home/me/developer/acme/api-server": "-home-me-developer-acme-api-server",
		"/home/me/café":                      "-home-me-caf-",
		"/home/me/a😀b":                       "-home-me-a--b",
	} {
		got, err := projectSlug(dir)
		if err != nil {
			t.Fatal(err)
		}
		if got != want {
			t.Errorf("projectSlug(%q) = %q, want %q", dir, got, want)
		}
	}
}

func TestQuoteSurvivesTheShell(t *testing.T) {
	tricky := `it's "$HOME" ; rm -rf / $(x)`
	out, err := exec.Command("sh", "-c", "printf %s "+quote(tricky)).Output()
	if err != nil {
		t.Fatal(err)
	}
	if string(out) != tricky {
		t.Errorf("got %q, want %q", out, tricky)
	}
}

func TestExpandHomeOnlyExpandsALeadingTilde(t *testing.T) {
	for in, want := range map[string]string{
		"~/.local/bin/claude": "/h/.local/bin/claude",
		"~":                   "/h",
		"/opt/claude~1/bin":   "/opt/claude~1/bin",
		"~other/bin":          "~other/bin",
	} {
		if got := expandHome(in, "/h"); got != want {
			t.Errorf("expandHome(%q) = %q, want %q", in, got, want)
		}
	}
}

func waitForFile(t *testing.T, path string, within time.Duration) bool {
	t.Helper()
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			return true
		}
		time.Sleep(100 * time.Millisecond)
	}
	return false
}

func startFakeClaude(t *testing.T, seconds string) *exec.Cmd {
	t.Helper()
	cmd := exec.Command("sleep", seconds)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	go cmd.Wait()
	t.Cleanup(func() { cmd.Process.Kill() })
	return cmd
}

func TestRetireClosesATerminalSessionAndRetiresItsTranscript(t *testing.T) {
	t.Setenv("REX_SESSION", "")
	transcript := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(transcript, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	claude := startFakeClaude(t, "60")
	if err := retire(armed{Host: "server", PID: claude.Process.Pid}, transcript); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, transcript+".handed-off-to-server", 5*time.Second) {
		t.Fatal("transcript was not retired after the session was closed")
	}
}

func TestRetireLeavesADesktopSessionRunningUntilItIsArchived(t *testing.T) {
	transcript := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(transcript, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	claude := startFakeClaude(t, "3")
	if err := retire(armed{Host: "server", PID: claude.Process.Pid, Desktop: true}, transcript); err != nil {
		t.Fatal(err)
	}
	if waitForFile(t, transcript+".handed-off-to-server", 2*time.Second) {
		t.Fatal("desktop session was stopped or its transcript moved before it archived itself")
	}
	if !waitForFile(t, transcript+".handed-off-to-server", 4*time.Second) {
		t.Fatal("transcript was not retired after the desktop session exited")
	}
}

func TestHandedOffAsksDesktopSessionsToArchiveThemselves(t *testing.T) {
	desktop := handedOff(armed{Host: "server", Desktop: true}, Result{ShownOn: []string{"Laptop"}})
	if desktop["decision"] != "block" || !strings.Contains(desktop["reason"], "archive_session") {
		t.Errorf("desktop output = %v", desktop)
	}
	terminal := handedOff(armed{Host: "server"}, Result{ShownOn: []string{"Laptop"}})
	if _, blocks := terminal["decision"]; blocks || terminal["systemMessage"] == "" {
		t.Errorf("terminal output = %v", terminal)
	}
}

func TestPullBringsWorkDoneOnTheServerBackToTheParkedLaptop(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), out, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	f.git(f.remote, "add", "edited.txt")
	f.write(f.remote, "server-new.txt", "new on the server")

	back, err := readWorkspace(f.remoteHost(), f.remote, []string{".env"})
	if err != nil {
		t.Fatal(err)
	}
	if err := pullGit(f.remoteHost(), f.machine(), back, f.local); err != nil {
		t.Fatalf("pulling back into the parked laptop failed: %v", err)
	}
	if got, want := f.git(f.local, "status", "--porcelain"), f.git(f.remote, "status", "--porcelain"); got != want {
		t.Errorf("laptop status after pulling:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.local, "edited.txt") + "|" + f.read(f.local, "server-new.txt"); got != "finished on the server|new on the server" {
		t.Errorf("laptop files after pulling = %q", got)
	}
	if refs := f.git(f.remote, "for-each-ref", "refs/cielago"); refs != "" {
		t.Errorf("the server kept refs after the pull: %s", refs)
	}
}

func TestLogFindsTheLastSessionSentToAHost(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	if _, err := lastHandoffTo("server"); err == nil {
		t.Fatal("found a session in an empty log")
	}
	for _, e := range []logEntry{
		{Time: time.Now(), Kind: "push", Session: "11111111-0000-0000-0000-000000000000", From: "laptop", To: "server", FromDir: "/a", ToDir: "/b"},
		{Time: time.Now(), Kind: "push", Session: "22222222-0000-0000-0000-000000000000", From: "laptop", To: "other", FromDir: "/a", ToDir: "/c"},
		{Time: time.Now(), Kind: "push", Session: "33333333-0000-0000-0000-000000000000", From: "laptop", To: "server", FromDir: "/d", ToDir: "/e"},
	} {
		if err := appendLog(e); err != nil {
			t.Fatal(err)
		}
	}
	got, err := lastHandoffTo("server")
	if err != nil || got != "33333333-0000-0000-0000-000000000000" {
		t.Fatalf("lastHandoffTo = %q, %v", got, err)
	}
	var out strings.Builder
	if err := status(&out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "33333333") || !strings.Contains(out.String(), "laptop:/d → server:/e") {
		t.Errorf("status output:\n%s", out.String())
	}
}

func TestClaudeVersionCheckFailsOldAndWarnsNew(t *testing.T) {
	for text, want := range map[string]string{
		"2.1.1 (Claude Code)":              "✗",
		testedClaude[1] + " (Claude Code)": "✓",
		"9.0.0 (Claude Code)":              "!",
	} {
		var out strings.Builder
		c := &checks{out: &out}
		c.claudeVersion("here", text)
		if !strings.HasPrefix(strings.TrimSpace(out.String()), want) {
			t.Errorf("%s gave %q, want %s", text, out.String(), want)
		}
	}
}

func TestTmuxLauncherOpensItsOwnWindowAndCleansUp(t *testing.T) {
	if _, err := exec.LookPath("tmux"); err != nil {
		t.Skip("tmux isn't installed")
	}
	f := newFixture(t)
	sockets, err := os.MkdirTemp("", "tm")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(sockets) })
	t.Setenv("TMUX_TMPDIR", sockets)
	t.Setenv("TMUX", "")
	t.Cleanup(func() { exec.Command("tmux", "kill-server").Run() })
	tm := Tmux{remote: f.remoteHost()}

	first, err := tm.Start("my.group", "api · claude", f.remote, []string{"sleep", "60"})
	if err != nil {
		t.Fatal(err)
	}
	if !first.SessionCreated || !strings.HasPrefix(first.Session, "$") || !strings.HasPrefix(first.Window, "@") || !strings.HasPrefix(first.Pane, "%") {
		t.Fatalf("first tab = %+v", first)
	}
	second, err := tm.Start("my.group", "web · claude", f.remote, []string{"sleep", "60"})
	if err != nil {
		t.Fatal(err)
	}
	if second.SessionCreated || second.Session != first.Session {
		t.Fatalf("second tab = %+v, want a window in the same session", second)
	}
	if err := tm.OpenProjectTab(second, "web", f.remote, "echo ready"); err != nil {
		t.Fatal(err)
	}
	windows, _ := exec.Command("tmux", "list-windows", "-t", first.Session, "-F", "#{window_name}").Output()
	if got := strings.Fields(strings.ReplaceAll(string(windows), " · ", "·")); strings.Join(got, ",") != "api·claude,web·claude,web" {
		t.Errorf("windows = %q", windows)
	}
	if err := tm.Discard(second); err != nil {
		t.Fatal(err)
	}
	if err := tm.Discard(first); err != nil {
		t.Fatal(err)
	}
	if out, _ := exec.Command("tmux", "list-sessions").CombinedOutput(); strings.Contains(string(out), "my.group") {
		t.Errorf("session left behind: %s", out)
	}
}

func TestACloneAtTheSamePathOnBothMachinesIsFound(t *testing.T) {
	f := newFixture(t)
	r := f.remoteHost()
	clones, err := listClones(r, []string{f.home + "/laptop"})
	if err != nil {
		t.Fatal(err)
	}
	if got, err := matchClone(clones, f.workspace(f.local), r.Name()); err != nil || got != f.local {
		t.Fatalf("matchClone = %q, %v; want the clone at the same path", got, err)
	}
}

func TestTheSameRootListedTwiceCountsOnce(t *testing.T) {
	f := newFixture(t)
	alias := filepath.Join(f.home, "dev-alias")
	if err := os.Symlink(filepath.Join(f.home, "developer"), alias); err != nil {
		t.Fatal(err)
	}
	clones, err := listClones(f.remoteHost(), []string{"~/developer", "~/dev-alias"})
	if err != nil {
		t.Fatal(err)
	}
	if got, err := matchClone(clones, f.workspace(f.local), "server"); err != nil || got != f.remote {
		t.Fatalf("matchClone = %q, %v; want one clone", got, err)
	}
}

func TestFindRemoteSessionPrefersTheNewestRecordAndRejectsBadIDs(t *testing.T) {
	f := newFixture(t)
	id := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
	f.write(f.home, ".claude/projects/-x/"+id+".jsonl", `{"cwd":"/old"}`+"\n")
	f.write(f.home, ".claude/sessions/1.json", `{"pid":1,"sessionId":"`+id+`","cwd":"/stale","startedAt":100}`)
	f.write(f.home, ".claude/sessions/2.json", `{"pid":2,"sessionId":"`+id+`","cwd":"/live","startedAt":200}`)
	s, err := findRemoteSession(f.remoteHost(), "server", id)
	if err != nil {
		t.Fatal(err)
	}
	if s.Live == nil || s.Live.PID != 2 || s.Cwd != "/live" {
		t.Errorf("session = %+v, want the newest record", s)
	}
	if _, err := findRemoteSession(f.remoteHost(), "server", "*"); err == nil || !strings.Contains(err.Error(), "not a Claude session id") {
		t.Errorf("err = %v, want an invalid id refusal", err)
	}
}

func TestStopAtTurnEndWaitsForTheTurnAndOnlyStopsClaude(t *testing.T) {
	f := newFixture(t)
	id := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
	fake := exec.Command("sh", "-c", "sleep 30; : claude --resume "+id)
	if err := fake.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- fake.Wait() }()
	pid := strconv.Itoa(fake.Process.Pid)
	f.write(f.home, ".claude/sessions/"+pid+".json", `{"pid":`+pid+`,"sessionId":"`+id+`","status":"busy"}`)
	go func() {
		time.Sleep(2500 * time.Millisecond)
		f.write(f.home, ".claude/sessions/"+pid+".json", `{"pid":`+pid+`,"sessionId":"`+id+`","status":"idle"}`)
	}()
	var out strings.Builder
	started := time.Now()
	if err := stopAtTurnEnd(f.remoteHost(), remoteSession{ID: id, Live: &liveSession{PID: fake.Process.Pid}}, &out); err != nil {
		t.Fatal(err)
	}
	if time.Since(started) < 2*time.Second || !strings.Contains(out.String(), "Waiting") {
		t.Errorf("stopped before the turn ended (%s, %q)", time.Since(started), out.String())
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		fake.Process.Kill()
		t.Fatal("the session process wasn't stopped")
	}

	other := exec.Command("sleep", "30")
	if err := other.Start(); err != nil {
		t.Fatal(err)
	}
	defer other.Process.Kill()
	if err := stopAtTurnEnd(f.remoteHost(), remoteSession{ID: id, Live: &liveSession{PID: other.Process.Pid}}, &out); err != nil {
		t.Fatal(err)
	}
	if err := other.Process.Signal(syscall.Signal(0)); err != nil {
		t.Error("a process that isn't Claude was stopped")
	}
}

func TestPullRefusesALocalCheckoutWithItsOwnChangesBeforeStoppingAnything(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "kept.txt", "laptop edit")
	ws := f.workspace(f.remote)
	if err := localCheckoutAccepts(f.machine(), Placement{Root: f.local}, ws); err == nil {
		t.Fatal("a laptop checkout with its own changes was accepted")
	}
	f.git(f.local, "checkout", "-q", "--", "kept.txt")
	if err := localCheckoutAccepts(f.machine(), Placement{Root: f.local}, ws); err != nil {
		t.Fatalf("a clean checkout was refused: %v", err)
	}
}
