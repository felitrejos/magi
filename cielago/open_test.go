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
	if got := strings.Join(names, ","); !strings.EqualFold(got, want) {
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

func TestOpenContinuesOnlyARealClaudeSession(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", "")
	dir := t.TempDir()
	for _, args := range [][]string{
		{"--json", "--resume", "not-an-id", dir},
		{"--json", "--codex", "--resume", "0f0f0f0f-1111-2222-3333-444444444444", dir},
	} {
		var out strings.Builder
		if err := openCommand(args, &out); err != errReported || !strings.Contains(out.String(), "not a Claude session id") {
			t.Errorf("%v: err = %v, out = %s", args, err, out.String())
		}
	}
}
