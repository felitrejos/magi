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
