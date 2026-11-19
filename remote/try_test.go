package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestTryRemovesOnlyItsOwnProject(t *testing.T) {
	f := newFixture(t)
	project := "magi-try-0123abcd"
	keep := filepath.Join(f.home, "laptop", "magi-try-keep")
	f.write(keep, "notes.txt", "mine")
	for _, dir := range []string{
		filepath.Join(f.home, "laptop", project),
		filepath.Join(f.home, "developer", project, project),
		filepath.Join(f.home, ".claude", "projects", "-home-laptop-"+project),
	} {
		f.write(dir, "notes.txt", "test")
	}
	host := Host{SSH: "fakehost", Claude: "claude", Roots: []string{"~/developer"}}
	if err := tryCleanup(f.machine(), []string{"~/laptop"}, host, "magi-try-keep", ""); err == nil {
		t.Fatal("removed a folder that isn't a magi try project")
	}
	if err := tryCleanup(f.machine(), []string{"~/laptop"}, host, project, ""); err != nil {
		t.Fatal(err)
	}
	for _, dir := range []string{filepath.Join(f.home, "laptop", project), filepath.Join(f.home, "developer", project), filepath.Join(f.home, ".claude", "projects", "-home-laptop-"+project)} {
		if _, err := os.Stat(dir); !os.IsNotExist(err) {
			t.Errorf("%s is still there", dir)
		}
	}
	if f.read(keep, "notes.txt") != "mine" {
		t.Error("another folder was touched")
	}
}
