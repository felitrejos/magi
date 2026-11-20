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

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

func (f *fixture) claudeTabHere() (sh.Tmux, sh.Tab) {
	f.t.Helper()
	f.isolatedTmux()
	tm := sh.Tmux{Machine: f.machine()}
	tab, err := tm.Start("acme", "api · claude", f.local, []string{"sleep", "60"}, false)
	if err != nil {
		f.t.Fatal(err)
	}
	return tm, tab
}

func tmuxWindows() string {
	out, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{window_name}").Output()
	return string(out)
}

func TestWhenTheOriginalCantBeClosedTheNewCopyIsStoppedAndNothingIsParked(t *testing.T) {
	f := newFixture(t)
	h := f.handoffToServer("0f0f0f0f-1111-2222-3333-555555555551")
	h.Retire = func() (int, error) { return 0, errors.New("no shell") }
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
	h := f.handoffToServer("0f0f0f0f-1111-2222-3333-555555555552")
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
	h := f.handoffToServer(id)
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
	local, err := sh.NewLocal(context.Background())
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
	if err := json.Unmarshal([]byte(out.String()), &reply); err != nil || reply["decision"] != "block" || !strings.Contains(reply["reason"], "ask it to move session 0f0f0f0f here") {
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
	const hookLimit = 200 * time.Second
	stop := config.Hooks["Stop"][0].Hooks[0]
	if stop.Command != `"${CLAUDE_PLUGIN_ROOT}/bin/magi" hook stop` || time.Duration(stop.Timeout)*time.Second != hookLimit {
		t.Fatalf("the Stop hook is %+v, want the plugin's magi hook stop with %s", stop, hookLimit)
	}
	worst := recoverBudget + prepareBudget + launchBudget + finishBudget + projectPanesLimit + cleanupBudget
	if worst > hookLimit-5*time.Second {
		t.Errorf("a handoff can take %s, more than the hook's %s", worst, hookLimit)
	}
}

func TestARecordedTmuxWindowIsOnlyClosedWhileItIsStillTheSameWindow(t *testing.T) {
	f := newFixture(t)
	tm, tab := f.claudeTabHere()
	if tab.Tag == "" {
		t.Fatalf("tab = %+v, want its tag", tab)
	}
	stale := tab
	stale.SessionCreated, stale.Tag = false, store.NewSessionID()
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

func TestARecordedTmuxWindowClosesAfterItsFirstPaneHasExited(t *testing.T) {
	f := newFixture(t)
	tm, tab := f.claudeTabHere()
	if _, err := tm.Split(tab.Pane, true, f.local, []string{"sleep", "60"}); err != nil {
		t.Fatal(err)
	}
	if err := exec.Command("tmux", "kill-pane", "-t", tab.Pane).Run(); err != nil {
		t.Fatal(err)
	}
	tab.SessionCreated = false
	if err := tm.Discard(tab); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(tmuxWindows(), "api · claude") {
		t.Error("the window outlived its first pane's process and stayed open with the project's panes")
	}
}

func TestRetireClosesTheRecordedTabWhenItsSessionPaneClosesFirst(t *testing.T) {
	f := newFixture(t)
	tm, tab := f.claudeTabHere()
	if _, err := tm.Split(tab.Pane, true, f.local, []string{"sleep", "60"}); err != nil {
		t.Fatal(err)
	}
	id := "0f0f0f0f-1111-2222-3333-999999999993"
	if err := recordTab(tm.Machine, id, tab); err != nil {
		t.Fatal(err)
	}
	out, err := exec.Command("tmux", "display-message", "-p", "-t", tab.Pane, "#{pane_pid}").Output()
	if err != nil {
		t.Fatal(err)
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(out)))
	if err != nil {
		t.Fatal(err)
	}
	transcript := filepath.Join(f.home, "s.jsonl")
	f.write(f.home, "s.jsonl", "{}")
	if _, err := retire(f.machine(), "laptop", id, transcript, pid); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, transcript+".handed-off-to-laptop", 10*time.Second) {
		t.Fatal("the transcript wasn't retired")
	}
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if !strings.Contains(tmuxWindows(), "api · claude") {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Error("retiring the session left its window open with the project's panes")
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
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := sh.Run(local, retireScript, fields[1], transcript, "server", "0f0f0f0f-1111-2222-3333-555555555556", fields[0], filepath.Join(f.home, "journal")); err != nil {
		t.Fatal(err)
	}
	if panes, _ := exec.Command("tmux", "list-panes", "-a", "-F", "#{pane_id}").Output(); strings.Contains(string(panes), fields[0]) {
		t.Errorf("the original's pane %s is still open", fields[0])
	}
	if _, err := os.Stat(transcript + ".handed-off-to-server"); err != nil {
		t.Errorf("the transcript wasn't marked as handed off: %v", err)
	}
}

func TestAPullOpensATmuxWindow(t *testing.T) {
	f := newFixture(t)
	f.taskHost()
	f.tool("claude", fakeClaude+"exec sleep 60\n")
	res := Pulled{Session: "0f0f0f0f-1111-2222-3333-555555555557", Name: "acme", ProjectRoot: f.local}
	res.ToDir = f.local
	if err := openPulledTab(&res, "back here"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(tmuxWindows(), "acme · claude") {
		t.Errorf("tmux windows = %q", tmuxWindows())
	}
}

func TestSessionsAndTheirDiffsAreReadOverSSH(t *testing.T) {
	f := newFixture(t)
	f.taskHost()
	id := "0f0f0f0f-1111-2222-3333-555555555558"
	f.write(filepath.Join(f.home, ".claude", "sessions"), strconv.Itoa(os.Getpid())+".json", `{"pid":`+strconv.Itoa(os.Getpid())+`,"sessionId":"`+id+`","cwd":"`+f.local+`","name":"fix login"}`)
	f.write(f.local, "kept.txt", "changed")
	f.write(f.local, "brand-new.txt", "new")
	all, err := everySession(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if text := describeSessions(all); !strings.Contains(text, "server:") {
		t.Errorf("sessions lack the server:\n%s", text)
	}
	if _, _, err := findAnySession(all, "fix login"); err == nil || !strings.Contains(err.Error(), "more than one") {
		t.Errorf("the same session on both fixture machines should be ambiguous, got %v", err)
	}
	m, s, err := findAnySession(all[1:], "0f0f0f0f")
	if err != nil {
		t.Fatal(err)
	}
	diff, err := sessionDiff(m.Machine, s)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(diff, "+changed") || !strings.Contains(diff, "new file: brand-new.txt") {
		t.Errorf("diff = %q", diff)
	}
	f.config("[hosts")
	if _, err := everySession(context.Background()); err == nil {
		t.Error("a broken config was read as no other machines")
	}
}

func TestASessionStartedInATabCanFindMagiEvenWithABarePath(t *testing.T) {
	f := newFixture(t)
	f.write(filepath.Join(f.home, ".local", "bin"), "magi", "#!/bin/sh\necho found\n")
	if err := os.Chmod(filepath.Join(f.home, ".local", "bin", "magi"), 0o755); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(sh.InDir(f.local, "sh", "-c", "magi")[0], sh.InDir(f.local, "sh", "-c", "magi")[1:]...)
	cmd.Env = []string{"HOME=" + f.home, "PATH=/usr/bin:/bin"}
	out, err := cmd.CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "found" {
		t.Fatalf("out = %q, %v", out, err)
	}
}

func TestHandingBackFindsTheMachineTheUserMeant(t *testing.T) {
	machines := []string{"felipes-macbook-pro", "homelab", "nas"}
	for wanted, want := range map[string]string{"felipes-macbook-pro": "felipes-macbook-pro", "Mac": "felipes-macbook-pro", "macbook": "felipes-macbook-pro", "a": "", "server": ""} {
		if got := matchMachine(machines, wanted); got != want {
			t.Errorf("matchMachine(%q) = %q, want %q", wanted, got, want)
		}
	}
}

func TestATmuxGroupNamedOutsideASCIIKeepsItsTabsInOneSession(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	tm := sh.Tmux{Machine: f.remoteHost()}
	first, err := tm.Start("grp·é", "api · claude", f.remote, []string{"sleep", "60"}, false)
	if err != nil {
		t.Fatal(err)
	}
	second, err := tm.Start("grp·é", "web · claude", f.remote, []string{"sleep", "60"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if !first.SessionCreated || second.SessionCreated || second.Session != first.Session {
		t.Errorf("first = %+v, second = %+v, want the second tab in the first one's session", first, second)
	}
}
