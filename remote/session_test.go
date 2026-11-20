package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
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

func TestWaitForRemoteControlWaitsForTheNewSessionsBridge(t *testing.T) {
	f := newFixture(t)
	launched := time.Now()
	waitBriefly := func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 1200*time.Millisecond)
		defer cancel()
		return waitForRemoteControl(f.remoteHost().With(ctx), "s1", launched)
	}
	if err := waitBriefly(); err == nil || !strings.Contains(err.Error(), "in time") {
		t.Fatalf("with no session files: err = %v, want a timeout rather than a script failure", err)
	}
	sessions := filepath.Join(f.home, ".claude", "sessions")
	stale := strconv.Itoa(startFakeClaude(t, "60").Process.Pid)
	f.write(sessions, stale+".json", `{"pid":`+stale+`,"sessionId":"s1","startedAt":1000,"bridgeSessionId":"old"}`)
	fresh := strconv.Itoa(startFakeClaude(t, "60").Process.Pid)
	started := strconv.FormatInt(launched.UnixMilli(), 10)
	f.write(sessions, fresh+".json", `{"pid":`+fresh+`,"sessionId":"s1","startedAt":`+started+`}`)
	if err := waitBriefly(); err == nil {
		t.Fatal("an earlier run's bridge, or a new session without one, counted as up")
	}

	f.write(sessions, fresh+".json", `{"pid":`+fresh+`,"sessionId":"s1","startedAt":`+started+`,"bridgeSessionId":"cse_1"}`)
	if err := waitForRemoteControl(f.remoteHost(), "s1", launched); err != nil {
		t.Fatalf("the new session's bridge wasn't seen: %v", err)
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
	if err := stopSession(f.remoteHost(), sessionID, false); err != nil {
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

func TestSessionStateComesFromClaudeAgentsAndTheLinkFromTheRegistry(t *testing.T) {
	f := newFixture(t)
	pid := strconv.Itoa(startFakeClaude(t, "60").Process.Pid)
	f.write(filepath.Join(f.home, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"0a0a0a0a-1111-2222-3333-444444444444","cwd":"/old","status":"idle","bridgeSessionId":"cse_1","procStart":"Wed Nov 18 18:37:12 2026"}`)
	f.tool("claude", `#!/bin/sh
[ "$1 $2" = "agents --json" ] || exit 1
cat <<'EOF'
[
  {
    "pid": `+pid+`,
    "cwd": "/srv/api",
    "startedAt": 1795027035220,
    "sessionId": "0a0a0a0a-1111-2222-3333-444444444444",
    "name": "fix login",
    "status": "busy"
  },
  {
    "pid": 1,
    "cwd": "/srv/web",
    "startedAt": 1795027035221,
    "sessionId": "0b0b0b0b-1111-2222-3333-444444444444",
    "name": "deploy",
    "status": "waiting",
    "waitingFor": "permission"
  }
]
EOF
`)
	all, err := everySession(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	text := describeSessions(all)
	for _, want := range []string{
		"  0a0a0a0a  fix login  busy  /srv/api  https://claude.ai/code/cse_1\n",
		"  0b0b0b0b  deploy  waiting (permission)  /srv/web\n",
		"Waiting on you: deploy (0b0b0b0b) on " + f.machine().Name() + " (permission).\n",
	} {
		if !strings.Contains(text, want) {
			t.Errorf("sessions lack %q:\n%s", want, text)
		}
	}
	out, err := sh.Run(f.machine(), platformScript, "claude")
	if err != nil {
		t.Fatal(err)
	}
	if m := parsePlatform("here", string(out)); m.Working != 1 {
		t.Errorf("the probe counted %d working sessions, want the one claude agents calls busy", m.Working)
	}
}

func TestFindSessionPrefersTheNewestRecordAndRejectsBadIDs(t *testing.T) {
	f := newFixture(t)
	id := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
	f.write(f.home, ".claude/projects/-x/"+id+".jsonl", `{"cwd":"/old"}`+"\n")
	older, newer := startFakeClaude(t, "60").Process.Pid, startFakeClaude(t, "60").Process.Pid
	f.write(f.home, ".claude/sessions/"+strconv.Itoa(older)+".json", `{"pid":`+strconv.Itoa(older)+`,"sessionId":"`+id+`","cwd":"/stale","startedAt":100}`)
	f.write(f.home, ".claude/sessions/"+strconv.Itoa(newer)+".json", `{"pid":`+strconv.Itoa(newer)+`,"sessionId":"`+id+`","cwd":"/live","startedAt":200}`)
	s, err := findSession(f.remoteHost(), id)
	if err != nil {
		t.Fatal(err)
	}
	if s.Live == nil || s.Live.PID != newer || s.Cwd != "/live" {
		t.Errorf("session = %+v, want the newest record", s)
	}
	if _, err := findSession(f.remoteHost(), "*"); err == nil || !strings.Contains(err.Error(), "is not a session id") {
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

func TestArmAsksAMachineThatWatchesToTakeTheSessionBack(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-555555555556"
	f.config("[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
	t.Setenv("CLAUDE_PID", "4242")
	heartbeat := filepath.Join(f.home, ".magi", "watchers", "Felipes-MacBook-Pro")
	f.write(filepath.Dir(heartbeat), filepath.Base(heartbeat), "")
	if err := arm("mac", id, "dev server on :3000", io.Discard); err != nil {
		t.Fatal(err)
	}
	path, _ := store.SessionPath("wanted", id, ".json")
	var w wanted
	if err := json.Unmarshal([]byte(f.read(filepath.Dir(path), filepath.Base(path))), &w); err != nil || w.To != "Felipes-MacBook-Pro" || w.PID != 4242 || w.Note != "dev server on :3000" {
		t.Errorf("wanted = %+v, %v", w, err)
	}
	var out strings.Builder
	if err := stopHook(strings.NewReader(`{"session_id":"`+id+`","background_tasks":[{"type":"shell","command":"npm run dev"}]}`), &out); err != nil || !strings.Contains(out.String(), "Waiting for Felipes-MacBook-Pro") {
		t.Fatalf("stop hook: %q, %v", out.String(), err)
	}
	if err := json.Unmarshal([]byte(f.read(filepath.Dir(path), filepath.Base(path))), &w); err != nil || !strings.Contains(w.Note, "npm run dev") {
		t.Errorf("after the turn the request is %+v, %v; want it kept with what was still running", w, err)
	}
	if err := arm("laptop", id, "", io.Discard); err == nil || !strings.Contains(err.Error(), "Felipes-MacBook-Pro") {
		t.Errorf("arm to a name no watcher matches gave %v; want it to name the machines that watch", err)
	}
	old := time.Now().Add(-time.Minute)
	if err := os.Chtimes(heartbeat, old, old); err != nil {
		t.Fatal(err)
	}
	if err := arm("mac", id, "", io.Discard); err == nil || !strings.Contains(err.Error(), "magi pull") {
		t.Errorf("arm with only a stale watcher gave %v", err)
	}
}

func TestArmNeedsAConfiguredHostAndTheClaudeProcess(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
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
	f.localSession(id, "busy")
	var out strings.Builder
	if err := arm("server", id, "dev server on :3000", &out); err != nil || !strings.Contains(out.String(), "What goes to server:\n  branch main") {
		t.Fatalf("arm: %q, %v", out.String(), err)
	}
	path, _ := store.SessionPath("armed", id, ".json")
	var a armed
	if err := json.Unmarshal([]byte(f.read(filepath.Dir(path), filepath.Base(path))), &a); err != nil || a.Host != "server" || a.PID != 4242 || a.Note != "dev server on :3000" {
		t.Errorf("armed = %+v, %v", a, err)
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
	path, _ := store.SessionPath("armed", id, ".json")
	f.write(filepath.Dir(path), filepath.Base(path), `{"host":"server","pid":1}`)
	t.Setenv("CLAUDE_PID", "2")
	if err := stopHook(strings.NewReader(input), &out); err != nil || out.Len() != 0 {
		t.Fatalf("a session armed by another process: output %q, err %v", out.String(), err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("the armed file outlived the turn it was meant for")
	}
}

func TestHandoffPromptCarriesTheNoteWhatWasStillRunningAndSubagentTranscripts(t *testing.T) {
	var input stopInput
	stop := `{"session_id":"s","background_tasks":[
		{"id":"b1","type":"shell","status":"running","description":"Start the dev server","command":"pnpm dev"},
		{"id":"a1","type":"subagent","status":"running","description":"Migrate the billing tests","agent_type":"general-purpose"}]}`
	if err := json.Unmarshal([]byte(stop), &input); err != nil {
		t.Fatal(err)
	}
	note := stillRunning("the subagent had done a.ts and b.ts", input.BackgroundTasks)
	p := handoffPrompt(Prompt{From: "laptop", To: "server", OldDir: "/a", NewDir: "/b", Git: true, Note: note, Sidecar: "/h/.claude/projects/-b/s1"})
	for _, want := range []string{
		"the subagent had done a.ts and b.ts; still running when the turn ended: Start the dev server (shell pnpm dev); Migrate the billing tests (subagent)",
		"/h/.claude/projects/-b/s1/subagents",
		"staged and unstaged",
		"confirming you're on server",
	} {
		if !strings.Contains(p, want) {
			t.Errorf("prompt is missing %q:\n%s", want, p)
		}
	}
	if got := stillRunning("", input.BackgroundTasks[:1]); got != "still running when the turn ended: Start the dev server (shell pnpm dev)" {
		t.Errorf("with no note of Claude's the note is %q", got)
	}
	if got := stillRunning("", nil); got != "" {
		t.Errorf("with nothing running the note is %q", got)
	}
}

func TestNothingWaitingSaysWhichMachinesWentUnchecked(t *testing.T) {
	text := describeSessions([]machineSessions{{Name: "laptop"}, {Name: "nas", Err: errors.New("timed out")}})
	if want := "Nothing that magi could check is waiting on you. Not checked: nas.\n"; !strings.HasPrefix(text, want) {
		t.Fatalf("sessions start %q, want %q", text, want)
	}
}

func TestWaitingOnASessionReturnsOnceItIsntBusy(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-dddddddddddd"
	proc, _ := f.localSession(id, "busy")
	done := make(chan string, 1)
	go func() {
		text, err := waitSession("0f0f0f0f", 50*time.Millisecond)
		done <- text + fmt.Sprint(err)
	}()
	select {
	case got := <-done:
		t.Fatalf("the wait returned while the session was busy: %s", got)
	case <-time.After(300 * time.Millisecond):
	}
	pid := strconv.Itoa(proc.Process.Pid)
	f.write(filepath.Join(f.home, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"`+id+`","cwd":"`+f.local+`","startedAt":1,"status":"idle"}`)
	select {
	case got := <-done:
		if want := "api (0f0f0f0f) on " + f.machine().Name() + " is idle.<nil>"; got != want {
			t.Errorf("wait = %q, want %q", got, want)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the wait didn't return once the session was idle")
	}
}
