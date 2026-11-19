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
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, LocalRoots: []string{"~/laptop"}},
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
	if err := saveStray(id, Stray{Host: "server", Tab: Tab{Session: "gone", Window: "@999"}}); err != nil {
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
	if tab.Tag == "" {
		t.Fatalf("tab = %+v, want its tag", tab)
	}
	stale := tab
	stale.SessionCreated, stale.Tag = false, newSessionID()
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
	if _, err := tm.Split(tab, tab.Pane, true, "pnpm dev", f.local, []string{"sleep", "60"}); err != nil {
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
	if _, err := tm.Split(tab, tab.Pane, true, "pnpm dev", f.local, []string{"sleep", "60"}); err != nil {
		t.Fatal(err)
	}
	id := "0f0f0f0f-1111-2222-3333-999999999993"
	if err := recordTab(local, id, tab); err != nil {
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
	if err := retire(armed{Host: "laptop", PID: pid}, id, transcript); err != nil {
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
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := sh(local, retireScript, fields[1], transcript, "server", "0f0f0f0f-1111-2222-3333-555555555556", fields[0]); err != nil {
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
	f.isolatedTmux()
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\nsleep 60\n")
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	res := Pulled{Session: "0f0f0f0f-1111-2222-3333-555555555557", Name: "acme", ProjectRoot: f.local}
	res.ToDir = f.local
	if err := openPulledTab(local, &res, "back here"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(tmuxWindows(), "acme · claude") {
		t.Errorf("tmux windows = %q", tmuxWindows())
	}
}

func TestSessionsAndTheirDiffsAreReadOverSSH(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	id := "0f0f0f0f-1111-2222-3333-555555555558"
	session := `{"pid":` + strconv.Itoa(os.Getpid()) + `,"sessionId":"` + id + `","cwd":"` + f.local + `","status":"waiting","waitingFor":"permission prompt","name":"fix login","bridgeSessionId":"session_01abc"}`
	f.write(filepath.Join(f.home, ".claude", "sessions"), strconv.Itoa(os.Getpid())+".json", session)
	f.write(filepath.Join(f.home, ".claude", "sessions"), "999999.json", `{"pid":999999,"sessionId":"gone","cwd":"/nowhere","status":"busy"}`)
	f.write(f.local, "kept.txt", "changed")
	f.write(f.local, "brand-new.txt", "new")
	all, err := everySession(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	text := describeSessions(all)
	for _, want := range []string{"0f0f0f0f  fix login  waiting (permission prompt)  " + f.local, "https://claude.ai/code/session_01abc", "server:"} {
		if !strings.Contains(text, want) {
			t.Errorf("sessions lack %q:\n%s", want, text)
		}
	}
	if strings.Contains(text, "gone") {
		t.Errorf("a session whose process ended is listed:\n%s", text)
	}
	if _, _, err := findAnySession(all, "fix login"); err == nil || !strings.Contains(err.Error(), "more than one") {
		t.Errorf("the same session on both fixture machines should be ambiguous, got %v", err)
	}
	m, s, err := findAnySession(all[:1], "0f0f0f0f")
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
}

func TestASessionStartedInATabCanFindMagiEvenWithABarePath(t *testing.T) {
	f := newFixture(t)
	f.write(filepath.Join(f.home, ".local", "bin"), "magi", "#!/bin/sh\necho found\n")
	if err := os.Chmod(filepath.Join(f.home, ".local", "bin", "magi"), 0o755); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(shellIn(f.local, "sh", "-c", "magi")[0], shellIn(f.local, "sh", "-c", "magi")[1:]...)
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

func TestLeftFromDesktopReadsTheLatestPush(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	id := "11111111-2222-3333-4444-555555555555"
	if leftFromDesktop(id) {
		t.Fatal("a session with no log entries left from Desktop")
	}
	for _, e := range []logEntry{
		{Kind: "push", Session: id, Desktop: true},
		{Kind: "pull", Session: id},
		{Kind: "push", Session: "other", Desktop: false},
	} {
		if err := appendLog(e); err != nil {
			t.Fatal(err)
		}
	}
	if !leftFromDesktop(id) {
		t.Fatal("the last push was from Desktop, but it wasn't seen")
	}
	if err := appendLog(logEntry{Kind: "push", Session: id}); err != nil {
		t.Fatal(err)
	}
	if leftFromDesktop(id) {
		t.Fatal("a later push from a terminal still counted as Desktop")
	}
}

func TestPromptHookPassesTheReturnNoteOnce(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	id := "11111111-2222-3333-4444-555555555555"
	path := returnNotePath(home, id)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("[magi] back on the Mac"), 0o600); err != nil {
		t.Fatal(err)
	}
	input := `{"session_id":"` + id + `"}`
	var out strings.Builder
	if err := promptHook(strings.NewReader(input), &out); err != nil {
		t.Fatal(err)
	}
	var got struct {
		HookSpecificOutput struct {
			HookEventName     string `json:"hookEventName"`
			AdditionalContext string `json:"additionalContext"`
		} `json:"hookSpecificOutput"`
	}
	if err := json.Unmarshal([]byte(out.String()), &got); err != nil {
		t.Fatal(err)
	}
	if got.HookSpecificOutput.HookEventName != "UserPromptSubmit" || got.HookSpecificOutput.AdditionalContext != "[magi] back on the Mac" {
		t.Fatalf("hook output = %s", out.String())
	}
	out.Reset()
	if err := promptHook(strings.NewReader(input), &out); err != nil {
		t.Fatal(err)
	}
	if out.Len() != 0 {
		t.Fatalf("the note was passed twice: %s", out.String())
	}
}

func TestATmuxGroupNamedOutsideASCIIKeepsItsTabsInOneSession(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	os.Unsetenv("TMUX")
	tm := Tmux{remote: f.remoteHost()}
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
