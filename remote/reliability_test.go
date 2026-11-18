package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func (f *fixture) readyHandoff(id string) Handoff {
	f.t.Helper()
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)
	slug, err := projectSlug(f.local)
	if err != nil {
		f.t.Fatal(err)
	}
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	return Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, Launcher: "tmux", LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}
}

func tmuxWindows() string {
	out, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{window_name}").Output()
	return string(out)
}

func TestWhenTheOriginalCantBeClosedTheNewCopyIsStoppedAndNothingIsParked(t *testing.T) {
	f := newFixture(t)
	h := f.readyHandoff("0f0f0f0f-1111-2222-3333-555555555551")
	h.Retire = func() error { return errors.New("no shell") }
	_, err := h.Run(context.Background())
	if err == nil || !strings.Contains(err.Error(), "the copy on server was stopped") {
		t.Fatalf("err = %v", err)
	}
	if windows := tmuxWindows(); strings.Contains(windows, "claude") {
		t.Errorf("the new copy's tab is still open: %q", windows)
	}
	if ref := f.git(f.local, "for-each-ref", parkedRef); ref != "" {
		t.Errorf("the checkout here stayed parked: %s", ref)
	}
}

func TestAnInterruptedHandoffStopsTheNewCopy(t *testing.T) {
	f := newFixture(t)
	h := f.readyHandoff("0f0f0f0f-1111-2222-3333-555555555552")
	ctx, cancel := context.WithCancel(context.Background())
	h.Progress = func(step string) {
		if step == "start" {
			cancel()
		}
	}
	_, err := h.Run(ctx)
	if err == nil {
		t.Fatal("an interrupted handoff reported success")
	}
	if windows := tmuxWindows(); strings.Contains(windows, "claude") {
		t.Errorf("the new copy's tab is still open: %q", windows)
	}
}

func TestAHandoffIsRefusedWhenTheSessionAlreadyRunsThere(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-555555555553"
	h := f.readyHandoff(id)
	running := exec.Command("sh", "-c", "sleep 30; :", "claude", "--resume", id)
	if err := running.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { running.Process.Kill(); running.Wait() })
	_, err := h.Run(context.Background())
	if err == nil || !strings.Contains(err.Error(), "already running on server") {
		t.Fatalf("err = %v", err)
	}
	if windows := tmuxWindows(); strings.Contains(windows, "claude") {
		t.Errorf("a refused handoff opened a tab: %q", windows)
	}
}

func TestASessionThatMovedRefusesNewInput(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-555555555554"
	input := `{"session_id":"` + id + `"}`
	var out strings.Builder
	if err := promptHook(strings.NewReader(input), &out); err != nil || out.String() != "" {
		t.Fatalf("before moving: %q, %v", out.String(), err)
	}
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if err := markMoved(local, id, "server"); err != nil {
		t.Fatal(err)
	}
	if err := promptHook(strings.NewReader(input), &out); err != nil {
		t.Fatal(err)
	}
	var reply map[string]string
	if err := json.Unmarshal([]byte(out.String()), &reply); err != nil || reply["decision"] != "block" || !strings.Contains(reply["reason"], "magi pull server") {
		t.Fatalf("reply = %q, %v", out.String(), err)
	}
	if err := clearMoved(local, id); err != nil {
		t.Fatal(err)
	}
	out.Reset()
	if err := promptHook(strings.NewReader(input), &out); err != nil || out.String() != "" {
		t.Errorf("after coming back: %q, %v", out.String(), err)
	}
	if _, err := os.Stat(filepath.Join(f.home, ".magi", "moved", id)); !os.IsNotExist(err) {
		t.Errorf("the marker is still there: %v", err)
	}
}

func TestACopyLeftByAFailedHandoffIsStoppedByStatus(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	if err := saveStray(id, Stray{Host: "server", Launcher: "tmux", Tab: Tab{Session: "gone", Window: "@999"}, Error: "ssh dropped"}); err != nil {
		t.Fatal(err)
	}
	var out strings.Builder
	if err := status(&out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "0f0f0f0f on server, stopped now") {
		t.Errorf("status said:\n%s", out.String())
	}
	if strays, err := loadStrays(); err != nil || len(strays) != 0 {
		t.Errorf("strays left = %v, %v", strays, err)
	}
}

func TestEveryHandoffStepFitsInsideTheHooksTimeLimit(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("..", "plugin", "hooks", "hooks.json"))
	if err != nil {
		t.Fatal(err)
	}
	var config struct {
		Hooks map[string][]struct {
			Hooks []struct {
				Command string `json:"command"`
				Timeout int    `json:"timeout"`
			} `json:"hooks"`
		} `json:"hooks"`
	}
	if err := json.Unmarshal(data, &config); err != nil {
		t.Fatal(err)
	}
	stop := config.Hooks["Stop"][0].Hooks[0]
	if stop.Command != "magi hook stop" || time.Duration(stop.Timeout)*time.Second != hookLimit {
		t.Fatalf("the Stop hook is %+v, want magi hook stop with %s", stop, hookLimit)
	}
	worst := prepareBudget + launchBudget + 10*time.Second + projectPanesLimit + 15*time.Second
	if worst >= hookLimit {
		t.Errorf("a handoff can take %s, more than the hook's %s", worst, hookLimit)
	}
	if moveHookLimit <= hookLimit {
		t.Errorf("magi move gives up after %s, before the hook's %s", moveHookLimit, hookLimit)
	}
}

func TestARecordedTmuxWindowIsOnlyClosedWhileItIsStillTheSameWindow(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	tm := Tmux{remote: local}
	tab, err := tm.Start("acme", "api · claude", f.local, []string{"sleep", "60"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if tab.PanePID == "" {
		t.Fatalf("tab = %+v, want its pane's process", tab)
	}
	stale := tab
	stale.SessionCreated, stale.PanePID = false, "1"
	if err := tm.Discard(stale); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(tmuxWindows(), "api · claude") {
		t.Fatal("a record for another window closed this one")
	}
	tab.SessionCreated = false
	if err := tm.Discard(tab); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(tmuxWindows(), "api · claude") {
		t.Error("the recorded window wasn't closed")
	}
}

func TestAfterAHandoffTheTmuxPaneTheSessionRanInCloses(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	out, err := exec.Command("tmux", "new-session", "-d", "-P", "-F", "#{pane_id}", "-s", "work", "sleep 60").Output()
	if err != nil {
		t.Fatal(err)
	}
	original := exec.Command("sleep", "60")
	if err := original.Start(); err != nil {
		t.Fatal(err)
	}
	go original.Wait()
	fields := []string{strings.TrimSpace(string(out)), strconv.Itoa(original.Process.Pid)}
	transcript := filepath.Join(f.home, "session.jsonl")
	f.write(f.home, "session.jsonl", "{}\n")
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := sh(local, retireScript, fields[1], "0", transcript, "server", "", "", "0f0f0f0f-1111-2222-3333-555555555556", fields[0]); err != nil {
		t.Fatal(err)
	}
	if panes, _ := exec.Command("tmux", "list-panes", "-a", "-F", "#{pane_id}").Output(); strings.Contains(string(panes), fields[0]) {
		t.Errorf("the original's pane %s is still open", fields[0])
	}
	if _, err := os.Stat(transcript + ".handed-off-to-server"); err != nil {
		t.Errorf("the transcript wasn't marked as handed off: %v", err)
	}
}

func TestAPullOpensATmuxWindowWhenThereIsNoRex(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	f.taskHost("")
	tmux, err := exec.LookPath("tmux")
	if err != nil {
		t.Fatal(err)
	}
	f.tool("claude", "#!/bin/sh\nsleep 60\n")
	if err := os.Symlink(tmux, filepath.Join(f.home, "bin", "tmux")); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", filepath.Join(f.home, "bin")+":/usr/bin:/bin")
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	res := Pulled{Session: "0f0f0f0f-1111-2222-3333-555555555557", Name: "acme", ProjectRoot: f.local}
	res.ToDir = f.local
	if err := openPulledTab(local, res, "back here"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(tmuxWindows(), "acme · claude") {
		t.Errorf("tmux windows = %q", tmuxWindows())
	}
}
