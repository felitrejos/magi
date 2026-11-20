package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/store"
)

func TestMain(m *testing.M) {
	if args := os.Getenv("MAGI_TEST_RUN"); args != "" {
		os.Args = append([]string{"magi"}, strings.Fields(args)...)
		main()
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func (f *fixture) armedToMove(id string) (*exec.Cmd, <-chan struct{}, string) {
	f.t.Helper()
	f.serverHome()
	f.moveHost(fakeClaudeThatAnswers)
	proc, transcript := f.localSession(id, "idle")
	exited := make(chan struct{})
	go func() { proc.Wait(); close(exited) }()
	pid := strconv.Itoa(proc.Process.Pid)
	f.t.Setenv("CLAUDE_PID", pid)
	path, _ := store.SessionPath("armed", id, ".json")
	f.write(filepath.Dir(path), filepath.Base(path), `{"host":"server","pid":`+pid+`}`)
	return proc, exited, `{"session_id":"` + id + `","transcript_path":"` + transcript + `","cwd":"` + f.local + `"}`
}

func (f *fixture) cutOff(id, step, stdin string, args ...string) {
	f.t.Helper()
	cmd := exec.Command(os.Args[0])
	cmd.Env = append(os.Environ(), "MAGI_TEST_RUN="+strings.Join(args, " "))
	cmd.Stdin = strings.NewReader(stdin)
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Start(); err != nil {
		f.t.Fatal(err)
	}
	exited := make(chan struct{})
	go func() { cmd.Wait(); close(exited) }()
	path, _ := movePath(id)
	for deadline := time.Now().Add(time.Minute); ; {
		select {
		case <-exited:
			f.t.Fatalf("magi %s ended before %s:\n%s", strings.Join(args, " "), step, out.String())
		case <-time.After(time.Millisecond):
		}
		if time.Now().After(deadline) {
			f.t.Fatalf("magi %s never reached %s", strings.Join(args, " "), step)
		}
		var j moveJournal
		if data, err := os.ReadFile(path); err == nil && json.Unmarshal(data, &j) == nil && j.Step == step {
			break
		}
	}
	cmd.Process.Signal(syscall.SIGSTOP)
	orphans := descendants(cmd.Process.Pid)
	cmd.Process.Kill()
	<-exited
	deadline := time.Now().Add(15 * time.Second)
	for _, pid := range orphans {
		for syscall.Kill(pid, 0) == nil && time.Now().Before(deadline) {
			time.Sleep(10 * time.Millisecond)
		}
	}
}

func descendants(pid int) []int {
	out, _ := exec.Command("pgrep", "-P", strconv.Itoa(pid)).Output()
	var all []int
	for _, field := range strings.Fields(string(out)) {
		child, _ := strconv.Atoi(field)
		all = append(append(all, child), descendants(child)...)
	}
	return all
}

func (f *fixture) transcript(home, dir, id string) string {
	f.t.Helper()
	path, err := claudeTranscript(home, dir, id)
	if err != nil {
		f.t.Fatal(err)
	}
	return path
}

func (f *fixture) appendTurn(path string) {
	f.t.Helper()
	file, err := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	if err != nil {
		f.t.Fatal(err)
	}
	defer file.Close()
	if _, err := file.WriteString(`{"type":"user","message":{"content":"keep going"}}` + "\n"); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) answered(id string) {
	f.t.Helper()
	path := f.transcript(filepath.Join(f.home, "server"), f.remote, id)
	for deadline := time.Now().Add(10 * time.Second); time.Now().Before(deadline); time.Sleep(50 * time.Millisecond) {
		if data, _ := os.ReadFile(path); bytes.Contains(data, []byte(`"type":"assistant"`)) {
			return
		}
	}
	f.t.Fatal("the copy on the server never answered")
}

func (f *fixture) status() string {
	f.t.Helper()
	var out strings.Builder
	if err := status(&out); err != nil {
		f.t.Fatal(err)
	}
	return out.String()
}

func (f *fixture) runsOnServer(id string) bool {
	f.t.Helper()
	sessions, err := liveSessions(f.remoteHost())
	if err != nil {
		f.t.Fatal(err)
	}
	return slices.ContainsFunc(sessions, func(s liveSession) bool { return s.SessionID == id })
}

func journaled(id string) bool {
	path, _ := movePath(id)
	_, err := os.Stat(path)
	return err == nil
}

func stillOpen(exited <-chan struct{}) bool {
	select {
	case <-exited:
		return false
	default:
		return true
	}
}

func TestStatusLeavesOneCopyWhereverTheStopHookWasKilled(t *testing.T) {
	for i, step := range []string{"sync", "launched", "finish", "retire"} {
		t.Run(step, func(t *testing.T) {
			f := newFixture(t)
			id := "0f0f0f0f-1111-2222-3333-12345678900" + strconv.Itoa(i)
			_, exited, hook := f.armedToMove(id)
			f.cutOff(id, step, hook, "hook", "stop")
			if step == "finish" {
				f.answered(id)
				if !stillOpen(exited) || !f.runsOnServer(id) {
					t.Fatal("killing the hook after launch should leave a copy on each machine")
				}
			}
			recorded := journaled(id)
			out := f.status()
			there := step == "retire"
			if here, server := stillOpen(exited), f.runsOnServer(id); here == there || server != there || journaled(id) {
				t.Fatalf("running here %v, on the server %v, still recorded %v\n%s", here, server, journaled(id), out)
			}
			moved := f.read(filepath.Join(f.home, ".magi", "moved"), id)
			if there {
				if moved != "server\n" || recorded && !strings.Contains(out, "The copy on server is the live one") {
					t.Errorf("moved marker %q\n%s", moved, out)
				}
				return
			}
			if moved != "<missing>" || !strings.Contains(out, "The copy here is the live one") {
				t.Errorf("moved marker %q\n%s", moved, out)
			}
		})
	}
}

func TestAMoveWhoseUndoCantReachTheHostIsSettledOnceItAnswers(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	id := "0f0f0f0f-1111-2222-3333-123456789010"
	h := f.handoffToServer(id)
	f.config("[hosts.server]\nssh = \"fakehost\"\nclaude = \"" + h.Host.Claude + "\"\nroots = [\"~/developer\"]\n[hosts.other]\nssh = \"otherhost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
	down := filepath.Join(f.home, "ssh-down")
	h.Progress = func(step string) {
		if step == "start" {
			f.write(f.home, "ssh-down", "")
		}
	}
	if _, err := h.Run(context.Background()); err == nil {
		t.Fatal("the move succeeded with the network down")
	}
	if out := f.status(); !strings.Contains(out, "magi can't reach server") || !journaled(id) || !strings.Contains(tmuxWindows(), "claude") {
		t.Fatalf("with the network down:\n%s", out)
	}
	var moved strings.Builder
	moveCommand([]string{id, "other", "--json"}, &moved)
	if events := moveEvents(t, moved.String()); !strings.Contains(events[len(events)-1].Message, "magi can't reach server") || !journaled(id) {
		t.Fatalf("a move to another host went ahead: %+v", events)
	}
	if err := os.Remove(down); err != nil {
		t.Fatal(err)
	}
	out := f.status()
	if journaled(id) || strings.Contains(tmuxWindows(), "claude") || !strings.Contains(out, "run claude --resume "+id+" in "+f.local) {
		t.Errorf("once the network is back, tmux has %q:\n%s", tmuxWindows(), out)
	}
}

func TestAPullCutOffAfterTheTranscriptArrivedIsFinishedByStatus(t *testing.T) {
	f := newFixture(t)
	server := f.serverHome()
	f.moveHost(fakeClaude)
	f.git(f.home, "clone", "-q", f.local, f.remote)
	f.git(f.remote, "remote", "set-url", "origin", "https://github.com/acme/api")
	id := "0f0f0f0f-1111-2222-3333-123456789011"
	there := f.sessionLeftOnServer(server, id)
	f.cutOff(id, "retire", "", "pull", "server", id)
	os.Remove(filepath.Join(f.home, ".magi", "handoffs.jsonl"))
	out := f.status()
	if journaled(id) || !strings.Contains(out, "The copy here is the live one") || !strings.Contains(out, "pull  0f0f0f0f") {
		t.Errorf("status:\n%s", out)
	}
	if f.read(filepath.Dir(there), filepath.Base(there)) != "<missing>" || f.read(filepath.Join(server, ".magi", "moved"), id) != f.machine().Name()+"\n" {
		t.Error("the copy on the server still takes prompts")
	}
	here := f.transcript(f.home, f.local, id)
	if f.read(filepath.Dir(here), filepath.Base(here)) == "<missing>" {
		t.Error("the transcript here is gone")
	}
	entries, _ := readLog()
	if pulls := slices.DeleteFunc(entries, func(e logEntry) bool { return e.Kind != "pull" }); len(pulls) != 1 {
		t.Errorf("pull log entries = %+v", pulls)
	}
}

func TestWhenBothCopiesWereUsedStatusWaitsUntilOneIsQuit(t *testing.T) {
	f := newFixture(t)
	id := "0f0f0f0f-1111-2222-3333-123456789012"
	proc, exited, hook := f.armedToMove(id)
	f.cutOff(id, "finish", hook, "hook", "stop")
	f.answered(id)
	here := f.transcript(f.home, f.local, id)
	f.appendTurn(here)
	f.appendTurn(f.transcript(filepath.Join(f.home, "server"), f.remote, id))
	if out := f.status(); !journaled(id) || !stillOpen(exited) || !f.runsOnServer(id) || !strings.Contains(out, "Quit the one you don't want") {
		t.Fatalf("with both copies used:\n%s", out)
	}
	proc.Process.Kill()
	<-exited
	out := f.status()
	if journaled(id) || !f.runsOnServer(id) || !strings.Contains(out, "kept in "+here+".handed-off-to-server") {
		t.Errorf("after quitting the copy here:\n%s", out)
	}
	if f.read(filepath.Dir(here), filepath.Base(here)+".handed-off-to-server") == "<missing>" {
		t.Error("the turns taken here were dropped")
	}
}

func TestAPullCutOffWhileWaitingForTheTurnKeepsTheSessionWhereItWas(t *testing.T) {
	remote := &sessionCopy{name: "server", used: true}
	here := &sessionCopy{here: true, used: true}
	r := recovery{j: &moveJournal{Kind: "pull", Step: "stop"}, from: remote, to: here, here: here, there: remote}
	if got := r.live(true); got != remote {
		t.Fatalf("live = %+v, want the copy on the server, since nothing was copied yet", got)
	}
}
