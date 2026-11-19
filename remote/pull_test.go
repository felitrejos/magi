package main

import (
	"context"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPullBringsWorkDoneOnTheServerBackToTheParkedLaptop(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := pushGit(f.machine(), f.remoteHost(), out, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out.Root, out); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	f.git(f.remote, "add", "edited.txt")
	f.write(f.remote, "server-new.txt", "new on the server")

	back, err := readWorkspace(f.remoteHost(), f.remote, []string{".env"}, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err := pullGit(f.remoteHost(), f.machine(), back, f.local); err != nil {
		t.Fatalf("pulling back into the parked laptop failed: %v", err)
	}
	if got, want := f.git(f.local, "status", "--porcelain"), f.git(f.remote, "status", "--porcelain"); got != want {
		t.Errorf("laptop status after pulling:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.local, "edited.txt") + "|" + f.read(f.local, "server-new.txt"); got != "finished on the server|new on the server" {
		t.Errorf("laptop files after pulling = %q", got)
	}
	if refs := f.git(f.remote, "for-each-ref", "refs/magi"); refs != "" {
		t.Errorf("the server kept refs after the pull: %s", refs)
	}
}

func TestPullRefusesALocalCheckoutWithItsOwnChangesBeforeStoppingAnything(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "kept.txt", "laptop edit")
	ws := f.workspace(f.remote)
	if err := checkoutAccepts(f.machine(), f.local, ws); err == nil {
		t.Fatal("a laptop checkout with its own changes was accepted")
	}
	f.git(f.local, "checkout", "-q", "--", "kept.txt")
	if err := checkoutAccepts(f.machine(), f.local, ws); err != nil {
		t.Fatalf("a clean checkout was refused: %v", err)
	}
}

func TestPullBringsTheSessionAndItsWorkBackFromTheServer(t *testing.T) {
	f := newFixture(t)
	f.messUp(f.local)
	out := f.workspace(f.local)
	if err := f.push(f.remoteHost(), out, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out.Root, out); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	id := "0f0f0f0f-1111-2222-3333-888888888888"
	slug, _ := projectSlug(f.remote)
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.remote+`"}`+"\n")

	res, err := Pull{HostName: "server", Host: Host{SSH: "fakehost", Claude: "claude", Codex: "codex", Roots: []string{"~/developer"}, LocalRoots: []string{"~/laptop"}, Secrets: []string{".env"}}, SessionID: id, Out: io.Discard}.Run()
	if err != nil {
		t.Fatalf("pull failed after %s: %v", res.Summary(), err)
	}
	if res.ToDir != f.local || res.Session != id || !strings.Contains(handoffPrompt(res.PromptInfo), "handed off from server") {
		t.Errorf("pulled = %+v", res)
	}
	if got, want := f.git(f.local, "status", "--porcelain"), f.git(f.remote, "status", "--porcelain"); got != want {
		t.Errorf("laptop status:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.local, "edited.txt"); got != "finished on the server" {
		t.Errorf("edited.txt on the laptop = %q", got)
	}
	localSlug, _ := projectSlug(f.local)
	if got := f.read(filepath.Join(f.home, ".claude", "projects", localSlug), id+".jsonl"); !strings.Contains(got, f.remote) {
		t.Errorf("transcript on the laptop = %q", got)
	}
	if got := f.read(filepath.Dir(transcript), id+".jsonl.handed-off-to-"+f.machine().Name()); got == "<missing>" {
		t.Error("the server's transcript wasn't retired")
	}
	if ref := f.git(f.remote, "rev-parse", "-q", "--verify", parkedRef); ref == "" {
		t.Error("the server checkout wasn't marked as handed off")
	}
}

func TestPullingASessionBackClosesItsTabOnTheServer(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	id := "0f0f0f0f-1111-2222-3333-999999999994"
	if _, err := f.handoffWithProject(id).Run(context.Background()); err != nil {
		t.Fatal(err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) == 0 {
		t.Fatal("the handoff opened no tab")
	}
	if err := closeRecordedTab(f.remoteHost(), id); err != nil {
		t.Fatal(err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) > 0 {
		t.Errorf("the session's tab outlived it: %s", windows)
	}
	if err := closeRecordedTab(f.remoteHost(), id); err != nil {
		t.Errorf("closing an already closed tab: %v", err)
	}
}

func TestPullRunsTheSetupHereAndTellsClaude(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, "proj")
	f.write(dir, "x", "")
	note := setUpPulledProject(&Project{Setup: "echo here > setup.txt", Services: []string{"pnpm dev"}}, dir, io.Discard)
	if got := strings.TrimSpace(f.read(dir, "setup.txt")); got != "here" {
		t.Errorf("setup.txt = %q", got)
	}
	if !strings.Contains(note, "ran here") || !strings.Contains(note, "pnpm dev") {
		t.Errorf("note = %q", note)
	}
	if note := setUpPulledProject(&Project{Setup: "exit 4"}, dir, io.Discard); !strings.Contains(note, "failed here") {
		t.Errorf("a failed setup gave note %q", note)
	}
}

func TestWatchTakesBackASessionTheServerAsksItTo(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\nsecrets = [\".env\"]\n")
	tmux, err := exec.LookPath("tmux")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(tmux, filepath.Join(f.home, "bin", "tmux")); err != nil {
		t.Fatal(err)
	}
	for _, tool := range []string{"git", "git-lfs"} {
		if path, err := exec.LookPath(tool); err == nil {
			if err := os.Symlink(path, filepath.Join(f.home, "bin", tool)); err != nil {
				t.Fatal(err)
			}
		}
	}
	t.Setenv("PATH", filepath.Join(f.home, "bin")+":/usr/bin:/bin")
	out := f.workspace(f.local)
	if err := f.push(f.remoteHost(), out, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), out.Root, out); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	id := "0f0f0f0f-1111-2222-3333-999999999999"
	stale := "0f0f0f0f-1111-2222-3333-999999999998"
	slug, _ := projectSlug(f.remote)
	f.write(filepath.Join(f.home, ".claude", "projects", slug), id+".jsonl", `{"cwd":"`+f.remote+`"}`+"\n")
	me := f.machine().Name()
	if err := writeWanted(id, wanted{To: me, PID: os.Getpid(), Note: "dev server on :3000"}); err != nil {
		t.Fatal(err)
	}
	if err := writeWanted(stale, wanted{To: me, PID: 999999}); err != nil {
		t.Fatal(err)
	}
	host, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var logged strings.Builder
	go followRequests(ctx, "server", host, me, log.New(&logged, "", 0))
	t.Cleanup(func() { t.Log(logged.String()) })
	wantedFile, _ := wantedPath(id)
	staleFile, _ := wantedPath(stale)
	for deadline := time.Now().Add(30 * time.Second); time.Now().Before(deadline); time.Sleep(200 * time.Millisecond) {
		if _, err := os.Stat(wantedFile); os.IsNotExist(err) {
			break
		}
	}
	if _, err := os.Stat(wantedFile); !os.IsNotExist(err) {
		t.Fatal("the request was never taken")
	}
	if got := f.read(f.local, "edited.txt"); got != "finished on the server" {
		t.Errorf("edited.txt on the laptop = %q", got)
	}
	if _, err := os.Stat(staleFile); !os.IsNotExist(err) {
		t.Error("a request whose session is gone was kept")
	}
	localSlug, _ := projectSlug(f.local)
	if got := f.read(filepath.Join(f.home, ".claude", "projects", localSlug), id+".jsonl"); !strings.Contains(got, f.remote) {
		t.Errorf("transcript on the laptop = %q", got)
	}
}
