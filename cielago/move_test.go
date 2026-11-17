package main

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func (f *fixture) moveHost() {
	f.t.Helper()
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)
	f.config(`local_roots = ["~/laptop"]

[hosts.server]
ssh = "fakehost"
claude = "` + claude + `"
roots = ["~/developer"]
secrets = [".env"]
launcher = "tmux"
`)
}

func (f *fixture) localSession(id, status string) (*exec.Cmd, string) {
	f.t.Helper()
	slug, err := projectSlug(f.local)
	if err != nil {
		f.t.Fatal(err)
	}
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	proc := exec.Command(f.tool("claude-session", "#!/bin/sh\nsleep 60\n"), "--resume", id)
	if err := proc.Start(); err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { proc.Process.Kill(); proc.Wait() })
	pid := strconv.Itoa(proc.Process.Pid)
	f.write(filepath.Join(f.home, ".claude", "sessions"), pid+".json",
		`{"pid":`+pid+`,"sessionId":"`+id+`","cwd":"`+f.local+`","startedAt":1,"kind":"interactive","entrypoint":"cli","status":"`+status+`"}`)
	return proc, transcript
}

func moveEvents(t *testing.T, out string) []moveReport {
	t.Helper()
	var events []moveReport
	for _, line := range strings.Split(strings.TrimSpace(out), "\n") {
		var r moveReport
		if err := json.Unmarshal([]byte(line), &r); err != nil {
			t.Fatalf("not a JSON line: %q", line)
		}
		events = append(events, r)
	}
	return events
}

func TestMoveRefusesUnknownSessions(t *testing.T) {
	f := newFixture(t)
	f.moveHost()
	fresh := "0f0f0f0f-1111-2222-3333-eeeeeeeeeeee"
	f.write(filepath.Join(f.home, ".claude", "sessions"), "4242.json", `{"pid":4242,"sessionId":"`+fresh+`","status":"idle"}`)
	for id, want := range map[string]string{
		fresh:                                  "session 0f0f0f0f hasn't started a conversation yet, so there's nothing to move",
		"0f0f0f0f-1111-2222-3333-777777777777": "no Claude Code or Codex session 0f0f0f0f-1111-2222-3333-777777777777",
		"not-an-id":                            "is not a session id",
	} {
		var out strings.Builder
		if err := moveCommand([]string{id, "server", "--json"}, &out); err != errReported {
			t.Errorf("%s: err = %v", id, err)
		}
		events := moveEvents(t, out.String())
		if last := events[len(events)-1]; last.OK || last.Event != "failed" || !strings.Contains(last.Message, want) {
			t.Errorf("%s: %+v, want %q", id, last, want)
		}
	}
}

func TestMovingAnIdleSessionStopsItAndHandsItOff(t *testing.T) {
	f := newFixture(t)
	f.moveHost()
	f.messUp(f.local)
	id := "0f0f0f0f-1111-2222-3333-888888888888"
	proc, transcript := f.localSession(id, "idle")
	exited := make(chan error, 1)
	go func() { exited <- proc.Wait() }()

	var out strings.Builder
	if err := moveCommand([]string{id, "server", "--json"}, &out); err != nil {
		t.Fatalf("move: %v\n%s", err, out.String())
	}
	events := moveEvents(t, out.String())
	if got := events[0].Event + " " + events[len(events)-1].Event; got != "moving moved" {
		t.Fatalf("events = %+v", events)
	}
	if last := events[len(events)-1]; !last.OK || last.ToDir != f.remote || last.Session != id || last.Host != "server" {
		t.Errorf("result = %+v", last)
	}
	select {
	case <-exited:
	case <-time.After(5 * time.Second):
		t.Fatal("the local session wasn't stopped")
	}
	if _, err := os.Stat(transcript + ".handed-off-to-server"); err != nil {
		t.Errorf("the transcript wasn't retired: %v", err)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("server status:\n%s\nwant:\n%s", got, want)
	}
	if entry, err := lastHandoffTo("server"); err != nil || entry != id {
		t.Errorf("log = %q, %v", entry, err)
	}
}

func TestMovingABusySessionWaitsForItsTurnAndReportsTheHandoff(t *testing.T) {
	f := newFixture(t)
	f.moveHost()
	id := "0f0f0f0f-1111-2222-3333-999999999999"
	proc, _ := f.localSession(id, "busy")
	path, _ := armedPath(id, ".json")

	done := make(chan string, 1)
	go func() {
		var out strings.Builder
		moveCommand([]string{id, "server", "--json"}, &out)
		done <- out.String()
	}()
	if !waitForFile(t, path, 5*time.Second) {
		t.Fatalf("the session wasn't armed: %s", <-done)
	}
	var a armed
	data, _ := os.ReadFile(path)
	if err := json.Unmarshal(data, &a); err != nil || a.Host != "server" || a.PID != proc.Process.Pid || !a.Notify || a.Desktop {
		t.Fatalf("armed = %+v, %v", a, err)
	}
	os.Remove(path)
	if err := writeMoveResult(id, moveReport{Event: "moved", OK: true, Message: "Moved."}); err != nil {
		t.Fatal(err)
	}
	select {
	case out := <-done:
		events := moveEvents(t, out)
		if got := events[0].Event + " " + events[len(events)-1].Event; got != "waiting moved" || events[1].Session != id {
			t.Errorf("events = %+v", events)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("move didn't report the hook's result")
	}
}

func TestAMoveThatStaysRemembersItUntilTheTurnEnds(t *testing.T) {
	f := newFixture(t)
	f.moveHost()
	id := "0f0f0f0f-1111-2222-3333-dddddddddddd"
	f.localSession(id, "busy")
	path, _ := armedPath(id, ".json")
	done := make(chan string, 1)
	go func() {
		var out strings.Builder
		moveCommand([]string{id, "server", "--json", "--stay"}, &out)
		done <- out.String()
	}()
	if !waitForFile(t, path, 5*time.Second) {
		t.Fatalf("the session wasn't armed: %s", <-done)
	}
	var a armed
	data, _ := os.ReadFile(path)
	if err := json.Unmarshal(data, &a); err != nil || !a.Stay {
		t.Fatalf("armed = %+v, %v", a, err)
	}
	os.Remove(path)
	if err := writeMoveResult(id, moveReport{Event: "moved", OK: true, Message: "Moved."}); err != nil {
		t.Fatal(err)
	}
	<-done
}

func TestMoveSaysWhenTheStopHookNeverRuns(t *testing.T) {
	f := newFixture(t)
	f.moveHost()
	moveHookGrace = 300 * time.Millisecond
	t.Cleanup(func() { moveHookGrace = 5 * time.Second })
	id := "0f0f0f0f-1111-2222-3333-aaaaaaaaaaaa"
	proc, _ := f.localSession(id, "busy")
	done := make(chan string, 1)
	go func() {
		var out strings.Builder
		moveCommand([]string{id, "server", "--json"}, &out)
		done <- out.String()
	}()
	path, _ := armedPath(id, ".json")
	if !waitForFile(t, path, 5*time.Second) {
		t.Fatal("the session wasn't armed")
	}
	pid := strconv.Itoa(proc.Process.Pid)
	f.write(filepath.Join(f.home, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"`+id+`","status":"idle"}`)
	select {
	case out := <-done:
		events := moveEvents(t, out)
		if last := events[len(events)-1]; last.OK || !strings.Contains(last.Message, "Stop hook didn't run") {
			t.Errorf("result = %+v", last)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("move kept waiting")
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("the session stayed armed")
	}
}

func TestTheStopHookReportsAFailedMoveToTheWaitingCommand(t *testing.T) {
	f := newFixture(t)
	f.config("[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\nlauncher = \"tmux\"\n")
	id := "0f0f0f0f-1111-2222-3333-bbbbbbbbbbbb"
	path, _ := armedPath(id, ".json")
	f.write(filepath.Dir(path), filepath.Base(path), `{"host":"server","pid":`+strconv.Itoa(os.Getppid())+`,"notify":true}`)
	input := `{"session_id":"` + id + `","transcript_path":"/nowhere/` + id + `.jsonl","cwd":"/nowhere"}`
	var out strings.Builder
	if err := stopHook(strings.NewReader(input), &out); err != nil {
		t.Fatal(err)
	}
	result, _ := armedPath(id, ".result.json")
	data, err := os.ReadFile(result)
	if err != nil {
		t.Fatal(err)
	}
	var r moveReport
	if err := json.Unmarshal(data, &r); err != nil || r.OK || !strings.Contains(r.Message, "keeps running here") {
		t.Errorf("result = %+v, %v", r, err)
	}
}

func TestASecondMoveOfTheSameSessionIsRefused(t *testing.T) {
	f := newFixture(t)
	f.moveHost()
	id := "0f0f0f0f-1111-2222-3333-cccccccccccc"
	f.localSession(id, "busy")
	unlock, err := lockMove(id)
	if err != nil {
		t.Fatal(err)
	}
	var out strings.Builder
	moveCommand([]string{"--json", id, "server"}, &out)
	if events := moveEvents(t, out.String()); !strings.Contains(events[len(events)-1].Message, "already being moved") {
		t.Errorf("events = %+v", events)
	}
	unlock()
	path, _ := armedPath(id, ".json")
	f.write(filepath.Dir(path), filepath.Base(path), `{"host":"other","pid":1}`)
	out.Reset()
	moveCommand([]string{id, "server", "--json"}, &out)
	if events := moveEvents(t, out.String()); !strings.Contains(events[len(events)-1].Message, "already armed") {
		t.Errorf("events = %+v", events)
	}
	if data, _ := os.ReadFile(path); !strings.Contains(string(data), "other") {
		t.Errorf("the earlier arming was overwritten: %s", data)
	}
}

func TestAReusedPidIsNotTakenForTheSession(t *testing.T) {
	f := newFixture(t)
	proc, _ := f.localSession("0f0f0f0f-1111-2222-3333-dddddddddddd", "idle")
	s := liveSession{PID: proc.Process.Pid}
	if alive, err := isClaudeProcess(f.machine(), s); err != nil || !alive {
		t.Fatalf("the session's own process: %v, %v", alive, err)
	}
	s.ProcStart = "Mon Jan  1 00:00:00 2001"
	if alive, err := isClaudeProcess(f.machine(), s); err != nil || alive {
		t.Errorf("a process that started at another time counted: %v, %v", alive, err)
	}
	if _, err := sh(f.machine(), sessionScript, "stop", strconv.Itoa(s.PID), "0f0f0f0f-1111-2222-3333-dddddddddddd", s.since()); err != nil {
		t.Fatal(err)
	}
	if err := proc.Process.Signal(syscall.Signal(0)); err != nil {
		t.Error("stop killed a process that only had the session's pid")
	}
}
