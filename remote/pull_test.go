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

	"github.com/felitrejos/magi/remote/internal/store"
)

func (f *fixture) sessionLeftOnServer(home, id string) string {
	f.t.Helper()
	f.messUp(f.local)
	ws := f.workspace(f.local)
	if err := f.push(f.remoteHost(), ws, Placement{Root: f.remote}); err != nil {
		f.t.Fatal(err)
	}
	if err := park(f.machine(), ws.Root, ws); err != nil {
		f.t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	transcript := f.transcript(home, f.remote, id)
	f.write(filepath.Dir(transcript), filepath.Base(transcript), `{"cwd":"`+f.remote+`"}`+"\n")
	return transcript
}

func TestPullBringsTheSessionAndItsWorkBackFromTheServer(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-888888888888"
	transcript := f.sessionLeftOnServer(f.home, id)

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
	if refs := f.git(f.remote, "for-each-ref", "refs/magi"); refs != "" {
		t.Errorf("the server kept refs after the pull: %s", refs)
	}
}

func TestPullingASessionBackClosesItsTabOnTheServer(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-999999999994"
	if _, err := f.handoffToServer(id).Run(context.Background()); err != nil {
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
	f.tool("claude", fakeClaudeThatStarts)
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\nsecrets = [\".env\"]\n")
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
	remote, err := newRemote(ctx, host)
	if err != nil {
		t.Fatal(err)
	}
	go newWatcher("server", remote, log.New(&logged, "", 0)).follow(me)
	t.Cleanup(func() { t.Log(logged.String()) })
	wantedFile, _ := store.SessionPath("wanted", id, ".json")
	staleFile, _ := store.SessionPath("wanted", stale, ".json")
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

func TestPullingASessionTakesTheMoveLock(t *testing.T) {
	f := newFixture(t)
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
	id := "0f0f0f0f-1111-2222-3333-777777777771"
	unlock, err := lockMove(id)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	if _, err := pullHere("server", id, "", io.Discard, false); err == nil || !strings.Contains(err.Error(), "already being moved") {
		t.Fatalf("err = %v, want the pull refused while the session is being moved", err)
	}
}
