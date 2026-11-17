package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

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
	if lines := strings.Count(f.read(f.home, ".magi/folders"), "\n"); lines != 1 {
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
