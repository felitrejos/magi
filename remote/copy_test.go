package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/felitrejos/magi/remote/internal/sh"
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
	got, ask, err := describeCopy(f.machine(), "server", Host{Secrets: []string{".env"}}, f.workspace(f.local), true)
	want := `What goes to server:
  branch main, its commits, and 1 changed file
  files git doesn't track yet: .magi.toml, a.txt, b.txt, c.txt, d.txt and 2 more
  changed and new files, up to 85 bytes
  secret files, by name only: .env
  from .magi.toml, nothing on this move: it isn't allowed here. It has setup "make", service "web", service "worker", ports 3000, 5173. A Claude session here can let it run on later moves by calling allow_project with dir ` + f.local + ` and sha256 ` + p.Hash + `.
  the conversation and its project memory
Nothing git ignores goes, apart from those secret files, and nothing that's running.`
	if err != nil || got != want || !ask {
		t.Errorf("describeCopy = %v, ask %v\n%s\nwant\n%s", err, ask, got, want)
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
	got, _, err := describeCopy(f.machine(), "server", Host{Secrets: []string{".env"}}, ws, true)
	if err != nil || !strings.Contains(got, "secret files, by name only: .env") || strings.Contains(got, "private-value") {
		t.Fatalf("block = %q, %v", got, err)
	}
}

func TestAnExcludedUntrackedFileStaysBehind(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "build/run.sh", "tracked")
	f.git(f.local, "add", "build/run.sh")
	f.git(f.local, "commit", "-q", "-m", "build")
	f.write(f.local, "build/run.sh", "edited")
	f.write(f.local, "build/out.o", "object")
	f.write(f.local, "web/dist/app.js", "bundle")
	f.write(f.local, "web/app.ts", "source")
	ws, err := readWorkspace(f.machine(), f.local, nil, []string{"dist", "build"})
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(ws.Untracked, " "); got != "web/app.ts" {
		t.Errorf("untracked = %s", got)
	}
	if got := f.git(f.local, "ls-tree", "-r", "--name-only", ws.Git.Worktree); strings.Contains(got, "dist") || strings.Contains(got, "out.o") || !strings.Contains(got, "web/app.ts") {
		t.Errorf("snapshot has:\n%s", got)
	}
	if got := f.git(f.local, "show", ws.Git.Worktree+":build/run.sh"); got != "edited" {
		t.Errorf("a tracked file under an excluded name went as %q", got)
	}
	if got, _, err := describeCopy(f.machine(), "server", Host{}, ws, false); err != nil || !strings.Contains(got, "\n  left out by exclude in magi's config: build/out.o and web/dist/\n") {
		t.Errorf("block = %v\n%s", err, got)
	}
	if err := park(f.machine(), f.local, ws); err != nil {
		t.Fatal(err)
	}
	if err := checkoutAccepts(f.machine(), f.local, ws); err != nil {
		t.Errorf("the checkout refused its own session coming back over the files exclude left here: %v", err)
	}
	g := ws.Git
	if _, err := sh.Run(f.machine(), checkoutScript, append([]string{"apply", f.local, g.Head, g.Worktree, g.Index, g.Branch}, ws.Exclude...)...); err != nil {
		t.Fatal(err)
	}
	for _, kept := range []string{"build/out.o", "web/dist/app.js"} {
		if _, err := os.Stat(filepath.Join(f.local, kept)); err != nil {
			t.Errorf("the session coming back removed %s, which exclude left here", kept)
		}
	}
}

func TestACopyNeedsAYesForSecretsUntrackedFilesOr50MB(t *testing.T) {
	git := &GitState{Branch: "main"}
	for _, c := range []struct {
		ws  Workspace
		ask bool
	}{
		{Workspace{Git: git, Size: 49_999_999}, false},
		{Workspace{Git: git, Size: 50_000_000}, true},
		{Workspace{Git: git, Untracked: []string{"notes.txt"}}, true},
		{Workspace{Git: git, Secrets: []string{".env"}}, true},
	} {
		if _, ask, err := describeCopy(nil, "server", Host{}, c.ws, false); err != nil || ask != c.ask {
			t.Errorf("%+v: ask %v, %v", c.ws, ask, err)
		}
	}
}
