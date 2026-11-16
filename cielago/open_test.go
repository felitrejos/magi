package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestOpenFindsProjectsInTheUsualFoldersWhenThereIsNoConfig(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", "")
	for _, dir := range []string{
		"code/stampr/stampr/.git",
		"code/stampr/stampr-ops/.git",
		"code/notes/.git",
		"code/.hidden/repo/.git",
		"code/a/b/too-deep/.git",
	} {
		if err := os.MkdirAll(filepath.Join(home, dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	roots, err := openRoots(local)
	if err != nil {
		t.Fatal(err)
	}
	found, err := knownProjects(roots)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, f := range found {
		names = append(names, strings.TrimPrefix(f, home+"/"))
	}
	want := "code/notes,code/stampr/stampr,code/stampr/stampr-ops"
	if got := strings.Join(names, ","); got != want {
		t.Errorf("found %s, want %s", got, want)
	}
	real, err := filepath.EvalSymlinks(home)
	if err != nil {
		t.Fatal(err)
	}
	group, err := localGroup(filepath.Join(real, "code", "stampr", "stampr-ops"), roots)
	if err != nil || group != "stampr" {
		t.Errorf("group = %q, %v", group, err)
	}
}

func TestOpenRefusesAnythingButAProjectFolderOrARealSession(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", "")
	for _, dir := range []string{"code/-evil/.git", "code/api/.git"} {
		if err := os.MkdirAll(filepath.Join(home, dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	outside := t.TempDir()
	for args, want := range map[string]string{
		"--json --resume not-an-id " + outside:                                    "not a Claude session id",
		"--json --codex --resume 0f0f0f0f-1111-2222-3333-444444444444 " + outside: "not a Claude session id",
		"--json " + outside: "isn't in one of this machine's project folders",
		"--json " + filepath.Join(home, "code", "-evil"): "can't name a session",
		"--json --list " + outside:                       "--list takes no folder",
	} {
		var out strings.Builder
		if err := openCommand(strings.Fields(args), &out); err != errReported || !strings.Contains(out.String(), want) {
			t.Errorf("%s: err = %v, out = %s", args, err, out.String())
		}
	}
	if err := openCommand([]string{"--json", "--resume"}, &strings.Builder{}); err == nil || !strings.Contains(err.Error(), "needs a session id") {
		t.Errorf("--resume without an id: %v", err)
	}
}

func TestAProjectReachedThroughASymlinkedRootKeepsItsGroup(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	real := filepath.Join(home, "elsewhere", "projects")
	if err := os.MkdirAll(filepath.Join(real, "stampr", "web", ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(real, filepath.Join(home, "projects")); err != nil {
		t.Fatal(err)
	}
	group, err := localGroup(filepath.Join(real, "stampr", "web"), []string{"~/projects"})
	if err != nil || group != "stampr" {
		t.Errorf("group = %q, %v", group, err)
	}
}
