package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestOldCielagoFoldersMoveToTheirMagiNames(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_CONFIG_HOME", "")
	for _, path := range []string{".cielago/tasks/x.json", ".config/cielago/config.toml"} {
		if err := os.MkdirAll(filepath.Join(home, filepath.Dir(path)), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(home, path), []byte("kept"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	moveOldFolders()
	for _, path := range []string{".magi/tasks/x.json", ".config/magi/config.toml"} {
		if data, err := os.ReadFile(filepath.Join(home, path)); err != nil || string(data) != "kept" {
			t.Errorf("%s = %q, %v", path, data, err)
		}
	}
	if err := os.MkdirAll(filepath.Join(home, ".cielago"), 0o700); err != nil {
		t.Fatal(err)
	}
	moveOldFolders()
	if _, err := os.Stat(filepath.Join(home, ".magi", "tasks", "x.json")); err != nil {
		t.Errorf("a second old folder replaced the new one: %v", err)
	}
}

func TestAProjectStillReadsItsOldCielagoFile(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, oldProjectFile), []byte("setup = \"make\"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	p, err := readProject(dir)
	if err != nil || p == nil || filepath.Base(p.Path) != oldProjectFile || p.Setup != "make" {
		t.Fatalf("project = %+v, %v", p, err)
	}
	if err := os.WriteFile(filepath.Join(dir, projectFile), []byte("setup = \"just\"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if p, err := readProject(dir); err != nil || p.Setup != "just" {
		t.Fatalf("the new file didn't win: %+v, %v", p, err)
	}
}
