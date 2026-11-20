package main

import (
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

func (f *fixture) moveHost(fake string) {
	f.t.Helper()
	f.isolatedTmux()
	claude := f.tool("claude", fake)
	f.config(`local_roots = ["~/laptop"]

[hosts.server]
ssh = "fakehost"
claude = "` + claude + `"
roots = ["~/developer"]
secrets = [".env"]
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
	proc := exec.Command(f.tool("claude-session", "#!/bin/sh\nsleep 60\n"))
	if err := proc.Start(); err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { proc.Process.Kill(); proc.Wait() })
	pid := strconv.Itoa(proc.Process.Pid)
	f.write(filepath.Join(f.home, ".claude", "sessions"), pid+".json",
		`{"pid":`+pid+`,"sessionId":"`+id+`","cwd":"`+f.local+`","startedAt":1,"status":"`+status+`"}`)
	return proc, transcript
}

func TestMoveRefusesUnknownSessions(t *testing.T) {
	f := newFixture(t)
	f.moveHost(fakeClaudeThatStarts)
	fresh := "0f0f0f0f-1111-2222-3333-eeeeeeeeeeee"
	pid := strconv.Itoa(startFakeClaude(t, "60").Process.Pid)
	f.write(filepath.Join(f.home, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"`+fresh+`","status":"idle"}`)
	for id, want := range map[string]string{
		fresh:                                  "session 0f0f0f0f hasn't started a conversation yet, so there's nothing to move",
		"0f0f0f0f-1111-2222-3333-777777777777": "no Claude Code session 0f0f0f0f-1111-2222-3333-777777777777",
		"not-an-id":                            "is not a session id",
	} {
		if err := (mover{session: id, host: "server", out: io.Discard}).run(); err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("%s: err = %v, want %q", id, err, want)
		}
	}
}

func TestASecondMoveOfTheSameSessionIsRefused(t *testing.T) {
	f := newFixture(t)
	f.moveHost(fakeClaudeThatStarts)
	id := "0f0f0f0f-1111-2222-3333-cccccccccccc"
	f.localSession(id, "busy")
	unlock, err := lockMove(id)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	if err := (mover{session: id, host: "server", out: io.Discard}).run(); err == nil || !strings.Contains(err.Error(), "already being moved") {
		t.Errorf("err = %v", err)
	}
}

func TestAReusedPidIsNotStopped(t *testing.T) {
	f := newFixture(t)
	proc, _ := f.localSession("0f0f0f0f-1111-2222-3333-dddddddddddd", "idle")
	s := liveSession{PID: proc.Process.Pid, fromRegistry: fromRegistry{ProcStart: "Mon Jan  1 00:00:00 2001"}}
	if _, err := sh.Run(f.machine(), sessionScript, "stop", strconv.Itoa(s.PID), "0f0f0f0f-1111-2222-3333-dddddddddddd", s.since(), "claude"); err != nil {
		t.Fatal(err)
	}
	if err := proc.Process.Signal(syscall.Signal(0)); err != nil {
		t.Error("stop killed a process that only had the session's pid")
	}
}

func TestMoveSessionMovesAnIdleSessionButNotABusyOneOrItsCaller(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	f.moveHost(fakeClaudeThatStarts)
	f.messUp(f.local)
	idle, busy, self := "1a1a1a1a-1111-2222-3333-444444444444", "2b2b2b2b-1111-2222-3333-444444444444", "3c3c3c3c-1111-2222-3333-444444444444"
	idleProc, transcript := f.localSession(idle, "idle")
	exited := make(chan error, 1)
	go func() { exited <- idleProc.Wait() }()
	busyProc, _ := f.localSession(busy, "busy")
	selfProc, _ := f.localSession(self, "idle")
	callerPID = selfProc.Process.Pid
	t.Cleanup(func() { callerPID = 0 })
	move := func(id string) (string, bool) {
		return f.call("move_session", map[string]any{"session": id[:8], "to": "server"})
	}
	confirmed := func(id string) (string, bool) {
		return f.call("move_session", map[string]any{"session": id[:8], "to": "server", "confirmed": true})
	}

	if text, failed := move(busy); !failed || !strings.Contains(text, "magi wait 2b2b2b2b") {
		t.Errorf("a busy session: %q", text)
	}
	armedPath, _ := store.SessionPath("armed", busy, ".json")
	if _, err := os.Stat(armedPath); !os.IsNotExist(err) || busyProc.Process.Signal(syscall.Signal(0)) != nil {
		t.Error("the busy session was armed or stopped")
	}
	if text, failed := move(self); !failed || !strings.Contains(text, "is this session; to move it, use the handoff skill") {
		t.Errorf("the caller's own session: %q", text)
	}
	if text, failed := move(idle); failed || !strings.Contains(text, "secret files, by name only: .env") || !strings.Contains(text, "confirmed set to true") || strings.Contains(text, "Moved") {
		t.Errorf("an idle session with secret files, before the user's yes:\n%s", text)
	}
	if idleProc.Process.Signal(syscall.Signal(0)) != nil {
		t.Fatal("the idle session was stopped before the user's yes")
	}
	if text, failed := confirmed(idle); failed || !strings.Contains(text, "What goes to server:") || !strings.Contains(text, "Moved session 1a1a1a1a to server") || !strings.Contains(text, "in the Claude apps at https://claude.ai/code/bridge") || strings.Contains(text, "tmux attach") {
		t.Errorf("an idle session:\n%s", text)
	}
	select {
	case <-exited:
	case <-time.After(5 * time.Second):
		t.Fatal("the idle session wasn't stopped")
	}
	if _, err := os.Stat(transcript + ".handed-off-to-server"); err != nil {
		t.Errorf("the transcript wasn't retired: %v", err)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("server status:\n%s\nwant:\n%s", got, want)
	}
	if entries, err := readLog(); err != nil || len(entries) != 1 || entries[0].Session != idle || entries[0].To != "server" {
		t.Errorf("log = %+v, %v", entries, err)
	}
}

func TestMoveSessionSendsASessionThatWasQuitHereToAHost(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	f.moveHost(fakeClaudeThatStarts)
	id := "7a7a7a7a-1111-2222-3333-444444444444"
	slug, err := projectSlug(f.local)
	if err != nil {
		t.Fatal(err)
	}
	f.write(filepath.Join(f.home, ".claude", "projects", slug), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	callerPID = os.Getpid()
	t.Cleanup(func() { callerPID = 0 })
	if text, failed := f.call("move_session", map[string]any{"session": id, "to": "server"}); failed || !strings.Contains(text, "Moved session 7a7a7a7a to server") {
		t.Fatalf("move_session:\n%s", text)
	}
}

func TestMoveSessionBringsASessionHereFromAHost(t *testing.T) {
	f := newFixture(t)
	server := f.serverHome()
	f.moveHost(fakeClaudeThatStarts)
	id := "4d4d4d4d-1111-2222-3333-444444444444"
	f.git(f.home, "clone", "-q", f.local, f.remote)
	f.git(f.remote, "remote", "set-url", "origin", "https://github.com/acme/api")
	f.sessionLeftOnServer(server, id)
	proc := exec.Command(f.tool("claude-session", "#!/bin/sh\nsleep 60\n"))
	if err := proc.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { proc.Process.Kill(); proc.Wait() })
	exited := make(chan error, 1)
	go func() { exited <- proc.Wait() }()
	pid := strconv.Itoa(proc.Process.Pid)
	f.write(filepath.Join(server, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"`+id+`","cwd":"`+f.remote+`","startedAt":1,"status":"idle"}`)

	text, failed := f.call("move_session", map[string]any{"session": id[:8], "to": f.machine().Name(), "confirmed": true})
	if failed || !strings.Contains(text, "Brought session 4d4d4d4d back from server to "+f.local) {
		t.Fatalf("move_session:\n%s", text)
	}
	select {
	case <-exited:
	case <-time.After(5 * time.Second):
		t.Fatal("the session on the server wasn't stopped")
	}
	if got := f.read(f.local, "edited.txt"); got != "finished on the server" {
		t.Errorf("edited.txt here = %q", got)
	}
	if !strings.Contains(tmuxWindows(), "claude") {
		t.Errorf("no tab opened here: %q", tmuxWindows())
	}
}

func TestMoveSessionBringsBackASessionThatWasQuitOnAHost(t *testing.T) {
	f := newFixture(t)
	server := f.serverHome()
	f.moveHost(fakeClaudeThatStarts)
	id := "5e5e5e5e-1111-2222-3333-444444444444"
	f.git(f.home, "clone", "-q", f.local, f.remote)
	f.git(f.remote, "remote", "set-url", "origin", "https://github.com/acme/api")
	f.sessionLeftOnServer(server, id)

	if text, failed := f.call("move_session", map[string]any{"session": id, "to": f.machine().Name(), "confirmed": true}); failed || !strings.Contains(text, "Brought session 5e5e5e5e back from server to "+f.local) {
		t.Fatalf("move_session:\n%s", text)
	}
	if got := f.read(f.local, "edited.txt"); got != "finished on the server" {
		t.Errorf("edited.txt here = %q", got)
	}
}

func TestAMoveThatFailsAfterStoppingTheSessionReopensItHere(t *testing.T) {
	f := newFixture(t)
	f.moveHost(fakeClaudeThatStarts)
	id := "6f6f6f6f-1111-2222-3333-444444444444"
	proc, _ := f.localSession(id, "idle")
	go proc.Wait()
	f.write(f.home, "ssh-down", "")
	text, failed := f.call("move_session", map[string]any{"session": id[:8], "to": "server"})
	if !failed || !strings.Contains(text, "magi reopened it here in a tmux window") {
		t.Fatalf("move_session:\n%s", text)
	}
	if !strings.Contains(tmuxWindows(), "claude") {
		t.Errorf("no tab opened here: %q", tmuxWindows())
	}
}

func TestAMoveReportKeepsTheRemoteControlLinkWhenTheTabIsShown(t *testing.T) {
	res := Result{Session: "0f0f0f0f-1111-2222-3333-444444444444", To: "server", Where: "open in tmux on server (1 attached)", Link: "https://claude.ai/code/cse_1"}
	if report := res.Report(); !strings.Contains(report, "open in tmux on server (1 attached)") || !strings.Contains(report, "https://claude.ai/code/cse_1") {
		t.Errorf("report = %q", report)
	}
}
