package main

import (
	"crypto/rand"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestACopyCutOffMidwayLeavesTheDestinationUntouched(t *testing.T) {
	f := newFixture(t)
	f.tool("ssh", strings.Replace(fakeSSH, `exec "${MAGI_TEST_LOGIN_SHELL:-sh}" -c "$*"`, `sh -c "$*" | head -c 1500000`, 1))
	src := filepath.Join(f.home, "developer", "notes")
	dst := filepath.Join(f.home, "laptop", "notes")
	big := make([]byte, 3000000)
	if _, err := rand.Read(big); err != nil {
		t.Fatal(err)
	}
	f.write(src, "big", string(big))
	f.write(dst, "big", "old")
	if err := transfer(f.remoteHost(), src, Archive{Paths: []string{"big"}}, f.machine(), dst); err == nil {
		t.Fatal("a copy cut off midway succeeded")
	}
	if got := f.read(dst, "big"); got != "old" {
		t.Errorf("big was replaced by %d bytes from a cut-off copy", len(got))
	}
	entries, err := os.ReadDir(dst)
	if err != nil || len(entries) != 1 {
		t.Errorf("the destination holds %v, %v; want only big", entries, err)
	}
}

func TestHandoffRefusesEditsHiddenByAssumeUnchangedOrSkipWorktree(t *testing.T) {
	f := newFixture(t)
	f.git(f.local, "update-index", "--assume-unchanged", "kept.txt")
	f.git(f.local, "update-index", "--skip-worktree", "edited.txt", "gone.txt")
	f.write(f.local, "kept.txt", "edited under assume-unchanged")
	f.write(f.local, "edited.txt", "edited under skip-worktree")
	if err := os.Remove(filepath.Join(f.local, "gone.txt")); err != nil {
		t.Fatal(err)
	}
	_, err := readWorkspace(f.machine(), f.local, nil, nil)
	if err == nil || !strings.Contains(err.Error(), "edited.txt\nkept.txt") || strings.Contains(err.Error(), "gone.txt") {
		t.Fatalf("err = %v, want a refusal naming kept.txt and edited.txt only", err)
	}
}

func TestASparseCheckoutWithFilesLeftOutIsHandedOff(t *testing.T) {
	f := newFixture(t)
	f.git(f.local, "update-index", "--skip-worktree", "gone.txt")
	if err := os.Remove(filepath.Join(f.local, "gone.txt")); err != nil {
		t.Fatal(err)
	}
	f.git(f.local, "update-index", "--assume-unchanged", "kept.txt")
	f.workspace(f.local)
}

func (f *fixture) shallowClone(dir string) {
	f.t.Helper()
	upstream := filepath.Join(f.home, "upstream", "api.git")
	f.git(f.home, "clone", "-q", "--bare", f.local, upstream)
	for _, n := range []string{"second", "third"} {
		f.write(f.local, n+".txt", n)
		f.git(f.local, "add", ".")
		f.git(f.local, "commit", "-q", "-m", n)
	}
	f.git(f.local, "push", "-q", upstream, "main")
	f.git(f.home, "clone", "-q", "--depth", "1", "file://"+upstream, dir)
	f.git(dir, "remote", "set-url", "origin", "git@github.com:acme/unreachable.git")
	f.write(dir, "wip.txt", "uncommitted")
}

func TestAShallowCloneHandsOffWhereItsHistoryIsMissing(t *testing.T) {
	f := newFixture(t)
	shallow := filepath.Join(f.home, "laptop", "shallow", "api")
	f.shallowClone(shallow)
	ws := f.workspace(shallow)
	r := f.remoteHost()
	where, err := place(r, ws, []string{"~/developer"}, "shallow", nil)
	if err != nil || where.NewFolder == "" {
		t.Fatalf("placed at %+v, %v", where, err)
	}
	if err := f.push(r, ws, where); err != nil {
		t.Fatalf("handing off a shallow clone: %v", err)
	}
	if got := f.git(where.Root, "rev-parse", "HEAD"); got != ws.Git.Head {
		t.Errorf("HEAD on the server = %s, want %s", got, ws.Git.Head)
	}
	if got := f.read(where.Root, "wip.txt"); got != "uncommitted" {
		t.Errorf("wip.txt = %q", got)
	}
}

func TestAShallowCloneComesBackWhereItsHistoryIsMissing(t *testing.T) {
	f := newFixture(t)
	shallow := filepath.Join(f.home, "developer", "shallow", "api")
	f.shallowClone(shallow)
	r := f.remoteHost()
	ws, err := readWorkspace(r, shallow, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	fresh := filepath.Join(f.home, "laptop", "shallow", "api")
	f.git(f.home, "init", "-q", "-b", "main", fresh)
	if err := pullGit(r, f.machine(), ws, fresh); err != nil {
		t.Fatalf("pulling a shallow clone back: %v", err)
	}
	if got := f.git(fresh, "rev-list", "HEAD"); got != ws.Git.Head {
		t.Errorf("history on the laptop = %s, want only %s", got, ws.Git.Head)
	}
	if got := f.read(fresh, "wip.txt"); got != "uncommitted" {
		t.Errorf("wip.txt = %q", got)
	}
}

func (f *fixture) rejectIndexRef() {
	f.t.Helper()
	f.tool("hook", "#!/bin/sh\n[ \"$1\" != refs/magi/index ]\n")
	if err := os.Rename(filepath.Join(f.home, "bin", "hook"), filepath.Join(f.remote, ".git", "hooks", "update")); err != nil {
		f.t.Fatal(err)
	}
}

func TestARefTheServerRejectsLeavesNoneOfTheSnapshotPushed(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "kept.txt", "edited")
	f.rejectIndexRef()
	if err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote); err == nil {
		t.Fatal("the push succeeded although the server rejected a ref")
	}
	if refs := f.git(f.remote, "for-each-ref", "refs/magi"); refs != "" {
		t.Errorf("the server kept part of the snapshot:\n%s", refs)
	}
}

func TestARejectedHandoffFromAShallowCloneLeavesNoneOfTheSnapshotPushed(t *testing.T) {
	f := newFixture(t)
	shallow := filepath.Join(f.home, "laptop", "shallow", "api")
	f.shallowClone(shallow)
	f.rejectIndexRef()
	if err := pushGit(f.machine(), f.remoteHost(), f.workspace(shallow), f.remote); err == nil {
		t.Fatal("the push succeeded although the server rejected a ref")
	}
	if refs := f.git(f.remote, "for-each-ref", "refs/magi"); refs != "" {
		t.Errorf("the server kept part of the snapshot:\n%s", refs)
	}
}

func TestARefThatCannotBeWrittenLeavesNoneOfTheSnapshotFetched(t *testing.T) {
	f := newFixture(t)
	f.write(f.remote, "kept.txt", "edited on the server")
	r := f.remoteHost()
	back, err := readWorkspace(r, f.remote, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	f.write(filepath.Join(f.local, ".git", "refs", "magi"), "index.lock", "")
	if err := pullGit(r, f.machine(), back, f.local); err == nil {
		t.Fatal("the fetch succeeded although a ref was locked")
	}
	if refs := f.git(f.local, "for-each-ref", "refs/magi"); refs != "" {
		t.Errorf("the laptop kept part of the snapshot:\n%s", refs)
	}
}

func TestACheckoutThatFailsLeavesTheBranchWhereItWas(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root writes into read-only folders")
	}
	f := newFixture(t)
	f.write(f.local, "locked/a.txt", "v1")
	f.git(f.local, "add", ".")
	f.git(f.local, "commit", "-q", "-m", "locked")
	f.git(f.remote, "pull", "-q", f.local, "main")
	before := f.git(f.remote, "rev-parse", "main")
	f.write(f.local, "kept.txt", "v2")
	f.git(f.local, "commit", "-q", "-am", "work")
	f.write(f.local, "locked/new.txt", "new")
	locked := filepath.Join(f.remote, "locked")
	if err := os.Chmod(locked, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Chmod(locked, 0o755) })
	if err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote); err == nil {
		t.Fatal("the handoff succeeded into a read-only folder")
	}
	if got := f.git(f.remote, "rev-parse", "main"); got != before {
		t.Errorf("main on the server moved to %s although the checkout failed; want %s", got, before)
	}
}

func TestACorruptLFSObjectIsCaughtAndRemoved(t *testing.T) {
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
	f.write(f.local, "weights.bin", "real weights")
	f.git(f.local, "add", ".")
	f.git(f.local, "commit", "-q", "-m", "weights")
	objects := lfsObjects(t, f.local)
	if len(objects) != 1 {
		t.Fatalf("laptop LFS objects = %v", objects)
	}
	if err := os.WriteFile(objects[0], []byte("bit rot"), 0o644); err != nil {
		t.Fatal(err)
	}
	err := pushGit(f.machine(), f.remoteHost(), f.workspace(f.local), f.remote)
	if err == nil || !strings.Contains(err.Error(), "corrupted") {
		t.Fatalf("err = %v, want the corrupt object reported", err)
	}
	if got := lfsObjects(t, f.remote); len(got) != 0 {
		t.Errorf("the corrupt object stayed on the server: %v", got)
	}
}

func lfsObjects(t *testing.T, repo string) []string {
	t.Helper()
	var objects []string
	err := filepath.WalkDir(filepath.Join(repo, ".git", "lfs", "objects"), func(path string, d fs.DirEntry, err error) error {
		if err == nil && d.Type().IsRegular() {
			objects = append(objects, path)
		}
		return err
	})
	if err != nil && !os.IsNotExist(err) {
		t.Fatal(err)
	}
	return objects
}
