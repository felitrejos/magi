package main

import (
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

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

func (f *fixture) secretArrivedAsAFile(name, body string) {
	f.t.Helper()
	path := filepath.Join(f.remote, name)
	info, err := os.Lstat(path)
	if err != nil {
		f.t.Fatal(err)
	}
	if !info.Mode().IsRegular() || info.Mode().Perm() != 0o600 {
		f.t.Errorf("%s on the server is %s, want a regular file with mode -rw-------", name, info.Mode())
	}
	if got := f.read(f.remote, name); got != body {
		f.t.Errorf("%s on the server = %q, want %q", name, got, body)
	}
}

func TestSecretsAreFoundPastUnreadableFoldersAndSymlinkedOnesArriveAsFiles(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root reads every folder")
	}
	f := newFixture(t)
	f.write(f.home, "shared/.env", "SHARED=1")
	if err := os.Chmod(filepath.Join(f.home, "shared/.env"), 0o600); err != nil {
		t.Fatal(err)
	}
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
	f.secretArrivedAsAFile(".env", "SHARED=1")
}

func TestASecretLinkedRelativelyOutsideTheCheckoutArrivesWithItsContent(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, "laptop/shared/.env.local", "LOCAL=1")
	if err := os.Chmod(filepath.Join(f.home, "laptop/shared/.env.local"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("../../shared/.env.local", filepath.Join(f.local, ".env.local")); err != nil {
		t.Fatal(err)
	}
	ws := f.workspace(f.local)
	if err := f.push(f.remoteHost(), ws, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	f.secretArrivedAsAFile(".env.local", "LOCAL=1")
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
