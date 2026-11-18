package main

import (
	"context"
	"encoding/json"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestSessionFilesFindTheSidecarAndExplainAMissingTranscript(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, ".claude", "projects", "-x")
	f.write(dir, "s1.jsonl", "{}")
	f.write(dir, "s1/subagents/a.jsonl", "{}")
	set, err := sessionFiles(f.machine(), filepath.Join(dir, "s1.jsonl"))
	if err != nil || strings.Join(set.Files, ",") != "s1.jsonl,s1" || !set.Subagents {
		t.Fatalf("set = %+v, %v", set, err)
	}
	for _, missing := range []string{filepath.Join(dir, "gone.jsonl"), filepath.Join(f.home, "no-such-project", "s1.jsonl")} {
		if _, err := sessionFiles(f.machine(), missing); err == nil || !strings.Contains(err.Error(), "isn't there") {
			t.Errorf("a missing transcript %s gave %v", missing, err)
		}
	}
}

func TestMemoryIsMergedNotReplaced(t *testing.T) {
	f := newFixture(t)
	local := filepath.Join(f.home, "laptop-memory")
	remote := filepath.Join(f.home, "host-memory")
	f.write(local, "MEMORY.md", "- [Deploys](deploys.md) — laptop wording\n- [Style](style.md) — tabs\n")
	f.write(local, "style.md", "tabs")
	f.write(local, "deploys.md", "laptop deploys")
	f.write(remote, "MEMORY.md", "- [Deploys](deploys.md) — host wording\n- [Hosts](hosts.md) — names\n")
	f.write(remote, "hosts.md", "names")
	f.write(remote, "deploys.md", "host deploys, newer")
	future := time.Now().Add(time.Hour)
	if err := os.Chtimes(filepath.Join(remote, "deploys.md"), future, future); err != nil {
		t.Fatal(err)
	}

	if err := syncMemory(f.machine(), local, f.remoteHost(), remote); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Stat(filepath.Join(remote, "MEMORY.md")); err != nil || info.Mode().Perm() != 0o600 {
		t.Errorf("MEMORY.md on the server: %v, %v; want it private", info.Mode(), err)
	}
	want := "- [Deploys](deploys.md) — laptop wording\n- [Style](style.md) — tabs\n- [Hosts](hosts.md) — names\n"
	if got := f.read(remote, "MEMORY.md"); got != want {
		t.Errorf("MEMORY.md =\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(remote, "style.md") + "|" + f.read(remote, "hosts.md") + "|" + f.read(remote, "deploys.md"); got != "tabs|names|host deploys, newer" {
		t.Errorf("memory files = %q", got)
	}
}

func TestHandoffPromptCarriesTheNoteAndSubagentTranscripts(t *testing.T) {
	p := handoffPrompt(Prompt{From: "laptop", To: "server", OldDir: "/a", NewDir: "/b", Git: true,
		Note: "dev server on :3000; a subagent was halfway through the API tests", Sidecar: "/h/.claude/projects/-b/s1"})
	for _, want := range []string{"dev server on :3000", "/h/.claude/projects/-b/s1/subagents", "staged and unstaged", "confirming you're on server"} {
		if !strings.Contains(p, want) {
			t.Errorf("prompt is missing %q:\n%s", want, p)
		}
	}
}

func TestWaitForRemoteControlIgnoresBrokenAndStaleSessionFiles(t *testing.T) {
	f := newFixture(t)
	launched := time.Now()
	sessions := filepath.Join(f.home, ".claude", "sessions")
	f.write(sessions, "1.json", "{not json")
	f.write(sessions, "2.json", `{"sessionId":"s1","startedAt":1000,"bridgeSessionId":"old"}`)

	ctx, cancel := context.WithTimeout(context.Background(), 1500*time.Millisecond)
	defer cancel()
	r := f.remoteHost()
	r.SetContext(ctx)
	if err := waitForRemoteControl(r, "s1", launched); err == nil {
		t.Fatal("a stale session file from an earlier run counted as the new session")
	}

	f.write(sessions, "3.json", `{"sessionId":"s1","startedAt":`+strconv.FormatInt(launched.UnixMilli(), 10)+`,"bridgeSessionId":"cse_1"}`)
	if err := waitForRemoteControl(f.remoteHost(), "s1", launched); err != nil {
		t.Fatal(err)
	}
}

func TestWaitForRemoteControlWorksWithNoSessionFiles(t *testing.T) {
	f := newFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 1200*time.Millisecond)
	defer cancel()
	r := f.remoteHost()
	r.SetContext(ctx)
	err := waitForRemoteControl(r, "s1", time.Now())
	if err == nil || !strings.Contains(err.Error(), "in time") {
		t.Fatalf("err = %v, want a timeout rather than a script failure", err)
	}
}

func TestIsTrustedAcceptsATrustedParent(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, ".claude.json", `{"projects":{"`+f.home+`/developer":{"hasTrustDialogAccepted":true}}}`)
	r := f.remoteHost()
	if trusted, err := isTrusted(r, f.remote); err != nil || !trusted {
		t.Fatalf("trusted = %v, err = %v; want a trusted parent to count", trusted, err)
	}
	if trusted, err := isTrusted(r, filepath.Join(f.home, "elsewhere")); err != nil || trusted {
		t.Fatalf("trusted = %v, err = %v; want an untrusted folder to report false", trusted, err)
	}
}

func TestTrustCarriesOverToTheOtherMachineAndKeepsEverythingElse(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, ".claude.json", `{"numStartups":7,"projects":{"`+f.home+`/Projects":{"hasTrustDialogAccepted":true,"allowedTools":["Bash"]}}}`)
	r := f.remoteHost()
	local := f.remoteHost()
	if err := carryTrust(local, r, filepath.Join(f.home, "Projects", "api"), f.remote); err != nil {
		t.Fatal(err)
	}
	if trusted, err := isTrusted(r, f.remote); err != nil || !trusted {
		t.Fatalf("trusted = %v, err = %v", trusted, err)
	}
	data, _ := os.ReadFile(filepath.Join(f.home, ".claude.json"))
	for _, kept := range []string{`"numStartups": 7`, `"allowedTools"`} {
		if !strings.Contains(string(data), kept) {
			t.Errorf("lost %s: %s", kept, data)
		}
	}
	if info, err := os.Stat(filepath.Join(f.home, ".claude.json")); err != nil || info.Mode().Perm() != 0o600 {
		t.Errorf("mode = %v, %v", info.Mode().Perm(), err)
	}
	untrusted := filepath.Join(f.home, "elsewhere")
	if err := carryTrust(local, r, untrusted, filepath.Join(f.home, "copy")); err != nil {
		t.Fatal(err)
	}
	if trusted, _ := isTrusted(r, filepath.Join(f.home, "copy")); trusted {
		t.Error("a folder not trusted here was trusted there")
	}
}

func TestStopSessionStopsTheSessionWithoutKillingItself(t *testing.T) {
	f := newFixture(t)
	sessionID := "11111111-2222-3333-4444-555555555555"
	fake := exec.Command("sh", "-c", "sleep 30; : --resume "+sessionID)
	if err := fake.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- fake.Wait() }()
	if err := stopSession(f.remoteHost(), sessionID); err != nil {
		t.Fatalf("stopSession failed: %v", err)
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		fake.Process.Kill()
		t.Fatal("the session process was not stopped")
	}
}

func TestProjectSlugMatchesClaudeFolderNames(t *testing.T) {
	for dir, want := range map[string]string{
		"/home/me/developer/acme/api-server": "-home-me-developer-acme-api-server",
		"/home/me/café":                      "-home-me-caf-",
		"/home/me/a😀b":                       "-home-me-a--b",
	} {
		got, err := projectSlug(dir)
		if err != nil {
			t.Fatal(err)
		}
		if got != want {
			t.Errorf("projectSlug(%q) = %q, want %q", dir, got, want)
		}
	}
}

func TestRetireClosesATerminalSessionAndRetiresItsTranscript(t *testing.T) {
	t.Setenv("REX_SESSION", "")
	transcript := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(transcript, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	claude := startFakeClaude(t, "60")
	if err := retire(armed{Host: "server", PID: claude.Process.Pid}, "0f0f0f0f-1111-2222-3333-aaaaaaaaaaaa", transcript); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, transcript+".handed-off-to-server", 5*time.Second) {
		t.Fatal("transcript was not retired after the session was closed")
	}
}

func TestRetireLeavesADesktopSessionRunningUntilItIsArchived(t *testing.T) {
	transcript := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(transcript, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	claude := startFakeClaude(t, "3")
	if err := retire(armed{Host: "server", PID: claude.Process.Pid, Desktop: true}, "0f0f0f0f-1111-2222-3333-bbbbbbbbbbbb", transcript); err != nil {
		t.Fatal(err)
	}
	if waitForFile(t, transcript+".handed-off-to-server", 2*time.Second) {
		t.Fatal("desktop session was stopped or its transcript moved before it archived itself")
	}
	if !waitForFile(t, transcript+".handed-off-to-server", 4*time.Second) {
		t.Fatal("transcript was not retired after the desktop session exited")
	}
}

func TestHandedOffAsksDesktopSessionsToArchiveThemselves(t *testing.T) {
	type hookOutput struct {
		Decision           string `json:"decision"`
		SystemMessage      string `json:"systemMessage"`
		HookSpecificOutput struct {
			HookEventName     string `json:"hookEventName"`
			AdditionalContext string `json:"additionalContext"`
		} `json:"hookSpecificOutput"`
	}
	printed := func(a armed) hookOutput {
		var out hookOutput
		data, err := json.Marshal(handedOff(a, Result{ShownOn: []string{"Laptop"}}))
		if err == nil {
			err = json.Unmarshal(data, &out)
		}
		if err != nil {
			t.Fatal(err)
		}
		return out
	}
	desktop := printed(armed{Host: "server", Desktop: true})
	if desktop.Decision != "" || desktop.HookSpecificOutput.HookEventName != "Stop" || !strings.Contains(desktop.HookSpecificOutput.AdditionalContext, "archive_session") {
		t.Errorf("desktop output = %+v, want Stop feedback asking it to archive itself, not an error", desktop)
	}
	terminal := printed(armed{Host: "server"})
	if terminal.Decision != "" || terminal.HookSpecificOutput.AdditionalContext != "" || terminal.SystemMessage == "" {
		t.Errorf("terminal output = %+v", terminal)
	}
}

func TestLogFindsTheLastSessionSentToAHost(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	if _, err := lastHandoffTo("server"); err == nil {
		t.Fatal("found a session in an empty log")
	}
	for _, e := range []logEntry{
		{Time: time.Now(), Kind: "push", Session: "11111111-0000-0000-0000-000000000000", From: "laptop", To: "server", FromDir: "/a", ToDir: "/b"},
		{Time: time.Now(), Kind: "push", Session: "22222222-0000-0000-0000-000000000000", From: "laptop", To: "other", FromDir: "/a", ToDir: "/c"},
		{Time: time.Now(), Kind: "push", Session: "33333333-0000-0000-0000-000000000000", From: "laptop", To: "server", FromDir: "/d", ToDir: "/e"},
	} {
		if err := appendLog(e); err != nil {
			t.Fatal(err)
		}
	}
	got, err := lastHandoffTo("server")
	if err != nil || got != "33333333-0000-0000-0000-000000000000" {
		t.Fatalf("lastHandoffTo = %q, %v", got, err)
	}
	if err := appendLog(logEntry{Time: time.Now(), Kind: "pull", Session: "33333333-0000-0000-0000-000000000000", From: "server", To: "laptop"}); err != nil {
		t.Fatal(err)
	}
	if _, err := lastHandoffTo("server"); err == nil || !strings.Contains(err.Error(), "already brought back") {
		t.Errorf("after pulling it back, lastHandoffTo gave %v", err)
	}
	var out strings.Builder
	if err := status(&out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "33333333") || !strings.Contains(out.String(), "laptop:/d → server:/e") {
		t.Errorf("status output:\n%s", out.String())
	}
}

func TestFindRemoteSessionPrefersTheNewestRecordAndRejectsBadIDs(t *testing.T) {
	f := newFixture(t)
	id := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
	f.write(f.home, ".claude/projects/-x/"+id+".jsonl", `{"cwd":"/old"}`+"\n")
	f.write(f.home, ".claude/sessions/1.json", `{"pid":1,"sessionId":"`+id+`","cwd":"/stale","startedAt":100}`)
	f.write(f.home, ".claude/sessions/2.json", `{"pid":2,"sessionId":"`+id+`","cwd":"/live","startedAt":200}`)
	s, err := findRemoteSession(f.remoteHost(), "server", id)
	if err != nil {
		t.Fatal(err)
	}
	if s.Live == nil || s.Live.PID != 2 || s.Cwd != "/live" {
		t.Errorf("session = %+v, want the newest record", s)
	}
	if _, err := findRemoteSession(f.remoteHost(), "server", "*"); err == nil || !strings.Contains(err.Error(), "is not a session id") {
		t.Errorf("err = %v, want an invalid id refusal", err)
	}
}

func TestStopAtTurnEndWaitsForTheTurnAndOnlyStopsClaude(t *testing.T) {
	f := newFixture(t)
	id := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
	fake := exec.Command("sh", "-c", "sleep 30; : claude --resume "+id)
	if err := fake.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- fake.Wait() }()
	pid := strconv.Itoa(fake.Process.Pid)
	f.write(f.home, ".claude/sessions/"+pid+".json", `{"pid":`+pid+`,"sessionId":"`+id+`","status":"busy"}`)
	idle := `{"pid":` + pid + `,"sessionId":"` + id + `","status":"idle"}`
	go func() {
		time.Sleep(2500 * time.Millisecond)
		if err := os.WriteFile(filepath.Join(f.home, ".claude/sessions", pid+".json"), []byte(idle), 0o644); err != nil {
			t.Error(err)
		}
	}()
	var out strings.Builder
	started := time.Now()
	if err := stopAtTurnEnd(f.remoteHost(), remoteSession{ID: id, Live: &liveSession{PID: fake.Process.Pid}}, &out); err != nil {
		t.Fatal(err)
	}
	if time.Since(started) < 2*time.Second || !strings.Contains(out.String(), "Waiting") {
		t.Errorf("stopped before the turn ended (%s, %q)", time.Since(started), out.String())
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		fake.Process.Kill()
		t.Fatal("the session process wasn't stopped")
	}

	other := exec.Command("sleep", "30")
	if err := other.Start(); err != nil {
		t.Fatal(err)
	}
	defer other.Process.Kill()
	if err := stopAtTurnEnd(f.remoteHost(), remoteSession{ID: id, Live: &liveSession{PID: other.Process.Pid}}, &out); err != nil {
		t.Fatal(err)
	}
	if err := other.Process.Signal(syscall.Signal(0)); err != nil {
		t.Error("a process that isn't Claude was stopped")
	}
}

func TestArmNeedsAConfiguredHostAndTheClaudeProcess(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\nlauncher = \"tmux\"\n")
	t.Setenv("CLAUDE_PID", "")
	if err := arm("server", id, "", io.Discard); err == nil || !strings.Contains(err.Error(), "CLAUDE_PID") {
		t.Errorf("arm outside Claude Code gave %v", err)
	}
	t.Setenv("CLAUDE_PID", "4242")
	if err := arm("nowhere", id, "", io.Discard); err == nil || !strings.Contains(err.Error(), "server") {
		t.Errorf("arm to an unknown host gave %v", err)
	}
	if err := arm("server", "not-an-id", "", io.Discard); err == nil {
		t.Error("arm accepted a malformed session id")
	}
	if err := arm("server", id, "dev server on :3000", io.Discard); err != nil {
		t.Fatal(err)
	}
	path, _ := armedPath(id, ".json")
	var a armed
	if err := json.Unmarshal([]byte(f.read(filepath.Dir(path), filepath.Base(path))), &a); err != nil || a.Host != "server" || a.PID != 4242 || a.Note != "dev server on :3000" {
		t.Errorf("armed = %+v, %v", a, err)
	}
}

func TestParentOfFindsThisProcesssParent(t *testing.T) {
	child := exec.Command("sleep", "5")
	if err := child.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { child.Process.Kill(); child.Wait() })
	if got, err := parentOf(child.Process.Pid); err != nil || got != os.Getpid() {
		t.Errorf("parentOf(child) = %d, %v; want %d", got, err, os.Getpid())
	}
}

func TestTheStopHookOnlyMovesASessionArmedFromItsOwnProcess(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-666666666666"
	input := `{"session_id":"` + id + `","transcript_path":"/nowhere.jsonl","cwd":"/nowhere"}`
	var out strings.Builder
	if err := stopHook(strings.NewReader(input), &out); err != nil || out.Len() != 0 {
		t.Fatalf("an unarmed session: output %q, err %v", out.String(), err)
	}
	path, _ := armedPath(id, ".json")
	f.write(filepath.Dir(path), filepath.Base(path), `{"host":"server","pid":1}`)
	if err := stopHook(strings.NewReader(input), &out); err != nil || out.Len() != 0 {
		t.Fatalf("a session armed by another process: output %q, err %v", out.String(), err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("the armed file outlived the turn it was meant for")
	}
}

func TestRetireClosesTheTabARecordedHandoffOpened(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	tab, err := Tmux{remote: local}.Start("acme", "api · claude", f.local, []string{"sleep", "60"}, false)
	if err != nil {
		t.Fatal(err)
	}
	id := "0f0f0f0f-1111-2222-3333-999999999995"
	if err := recordTab(local, id, "tmux", tab); err != nil {
		t.Fatal(err)
	}
	transcript := filepath.Join(f.home, "s.jsonl")
	f.write(f.home, "s.jsonl", "{}")
	claude := startFakeClaude(t, "60")
	if err := retire(armed{Host: "laptop", PID: claude.Process.Pid}, id, transcript); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, transcript+".handed-off-to-laptop", 5*time.Second) {
		t.Fatal("the transcript wasn't retired")
	}
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) == 0 {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	t.Error("retiring the session left its recorded tab open")
}
