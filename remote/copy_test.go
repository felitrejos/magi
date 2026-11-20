package main

import (
	"path/filepath"
	"strings"
	"testing"
)

func TestTheBlockNamesWhatGoesAndNeverASecretsValue(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "edited.txt", "v2")
	for _, name := range []string{"a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"} {
		f.write(f.local, name, "new")
	}
	f.write(f.local, ".env", "TOKEN=never-shown-4521")
	f.write(f.local, projectFile, "setup = \"make\"\nservices = [\"web\", \"worker\"]\nports = [3000, 5173]\n")
	p, err := readProject(f.local)
	if err != nil {
		t.Fatal(err)
	}
	got, err := describeCopy(f.machine(), "server", Host{Secrets: []string{".env"}}, f.workspace(f.local), true, false)
	want := `What goes to server:
  branch main, its commits, and 1 changed file
  files git doesn't track yet: .magi.toml, a.txt, b.txt, c.txt, d.txt and 2 more
  secret files, by name only: .env
  from .magi.toml, nothing on this move: it isn't allowed here. It has setup "make", service "web", service "worker", ports 3000, 5173. A Claude session here can let it run on later moves by calling allow_project with dir ` + f.local + ` and sha256 ` + p.Hash + `.
  the conversation and its project memory
Nothing git ignores goes, apart from those secret files, and nothing that's running.`
	if err != nil || got != want {
		t.Errorf("describeCopy = %v\n%s\nwant\n%s", err, got, want)
	}
}

func TestAPlainFolderNamesItsSecretsButNeverTheirValues(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, "laptop", "notes")
	f.write(dir, "a.txt", "a")
	f.write(dir, ".env", "K=private-value")
	ws, err := readWorkspace(f.machine(), dir, []string{".env"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	got, err := describeCopy(f.machine(), "server", Host{Secrets: []string{".env"}}, ws, true, false)
	if err != nil || !strings.Contains(got, "secret files, by name only: .env") || strings.Contains(got, "private-value") {
		t.Fatalf("block = %q, %v", got, err)
	}
}
