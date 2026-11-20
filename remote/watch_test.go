package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func readLive(t *testing.T, name string) liveMachine {
	t.Helper()
	path, err := livePath(name)
	if err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	var live liveMachine
	json.Unmarshal(data, &live)
	return live
}

const burn = `i=0; while [ "$i" -lt 300000 ]; do i=$((i + 1)); done`

func (f *fixture) worker(script string) int {
	f.t.Helper()
	done := filepath.Join(f.t.TempDir(), "done")
	cmd := exec.Command("sh", "-c", script, "sh", done)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL); cmd.Wait() })
	if !waitForFile(f.t, done, 30*time.Second) {
		f.t.Fatal("the worker never finished its work")
	}
	return cmd.Process.Pid
}

func usedBy(t *testing.T, stats string, pid int) (seconds float64, memoryMB int) {
	t.Helper()
	for _, field := range strings.Fields(stats) {
		if _, err := fmt.Sscanf(field, fmt.Sprintf("%d:%%g:%%d", pid), &seconds, &memoryMB); err == nil {
			return seconds, memoryMB
		}
	}
	t.Fatalf("stats line %q lacks process %d", stats, pid)
	return 0, 0
}

func TestWatchStreamsWhatChanges(t *testing.T) {
	f := newFixture(t)
	server := f.serverHome()
	pid := f.worker(`sh -c '` + burn + `; : > "$0"; exec sleep 60' "$1" & wait`)
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	busySince := time.Now().Add(-time.Minute).Truncate(time.Millisecond)
	session := func(status string, at time.Time) {
		f.write(filepath.Join(server, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"%s","cwd":"/srv/api","name":"api","status":"%s","statusUpdatedAt":%d,"bridgeSessionId":"session_42"}`, pid, id, status, at.UnixMilli()))
	}
	session("busy", busySince)
	f.write(filepath.Join(server, ".magi", "tabs"), id, "$3\n@7\nmagi\n")
	r := f.remoteHost()
	w := newWatcher("server", "me@server", r, log.New(io.Discard, "", 0), "")
	cmd := r.Command(watchScript, "", "claude")
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	lines := make(chan string)
	go func() {
		for s := bufio.NewScanner(stdout); s.Scan(); {
			lines <- s.Text()
		}
	}()
	var heard []string
	await := func(kind string, n int) {
		t.Helper()
		for timeout := time.After(15 * time.Second); n > 0; {
			select {
			case line := <-lines:
				heard = append(heard, line)
				w.hear(line, time.Now())
				if strings.HasPrefix(line, kind) {
					n--
				}
			case <-timeout:
				t.Fatalf("no %s line; heard %q", kind, heard)
			}
		}
	}
	await("sessions", 1)
	await("stats", 1)
	if used, memory := usedBy(t, heard[len(heard)-1], pid); used < 0.1 || memory < 1 {
		t.Errorf("stats line = %q, want the session's tree summed with its child's CPU and memory", heard[len(heard)-1])
	}
	now := time.Now()
	w.hear(fmt.Sprintf("stats 10 1.5 100 200 300 400 %d:10:150", pid), now)
	w.hear(fmt.Sprintf("stats 10 1.5 100 200 300 400 %d:13:150", pid), now.Add(3*time.Second))
	live := readLive(t, "server")
	if len(live.Sessions) != 1 {
		t.Fatalf("live sessions = %+v", live.Sessions)
	}
	s := live.Sessions[0]
	if s.ID != id || s.State != "busy" || !s.Since.Equal(busySince) || s.Title != "api" || s.Folder != "/srv/api" {
		t.Errorf("live session = %+v, want %s busy since %s", s, id, busySince)
	}
	if s.RemoteControl != "https://claude.ai/code/session_42" || s.Tmux != "@7" || live.SSH != "me@server" {
		t.Errorf("live session opens through %q and tmux window %q, machine over ssh %q", s.RemoteControl, s.Tmux, live.SSH)
	}
	if s.MemoryMB != 150 || s.CPU < 90 || s.CPU > 110 {
		t.Errorf("session uses %d%% CPU and %d MB, want about 100%% and 150 MB", s.CPU, s.MemoryMB)
	}
	if !live.Answering || live.Cores != 10 || live.Memory.TotalMB != 200 || live.Disk.TotalMB != 400 {
		t.Errorf("live machine = %+v", live)
	}
	idleSince := time.Now().Truncate(time.Millisecond)
	session("idle", idleSince)
	await("sessions", 1)
	if s := readLive(t, "server").Sessions[0]; s.State != "idle" || !s.Since.Equal(idleSince) {
		t.Errorf("after going idle the live session = %+v", s)
	}
	var sessionLines []string
	for _, line := range heard {
		if strings.HasPrefix(line, "claude") {
			sessionLines = append(sessionLines, line)
		}
	}
	if len(sessionLines) != 2 || !strings.Contains(sessionLines[0], `"busy"`) || !strings.Contains(sessionLines[1], `"idle"`) {
		t.Errorf("session lines = %q, want one snapshot when busy and one when idle", sessionLines)
	}
}

func TestWatchCountsTheCPUOfChildrenThatExited(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("only Linux shows other processes how much CPU their exited children used")
	}
	f := newFixture(t)
	server := f.serverHome()
	pid := f.worker(`(` + burn + `); : > "$1"; sleep 60; :`)
	f.write(filepath.Join(server, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"0f0f0f0f-1111-2222-3333-555555555556","cwd":"/srv/api","status":"idle"}`, pid))
	cmd := f.remoteHost().Command(watchScript, "", "claude")
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	for lines := bufio.NewScanner(stdout); lines.Scan(); {
		if strings.HasPrefix(lines.Text(), "stats") {
			if used, _ := usedBy(t, lines.Text(), pid); used < 0.1 {
				t.Errorf("stats line = %q, want the session's CPU to count its child that exited", lines.Text())
			}
			return
		}
	}
	t.Fatal("the watcher never reported stats")
}

func TestWatchMarksAMachineThatStopsAnswering(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	r, err := newRemote(ctx, Host{SSH: "fakehost", Claude: "claude"})
	if err != nil {
		t.Fatal(err)
	}
	stopped := make(chan error)
	go func() { stopped <- newWatcher("server", "", r, log.New(io.Discard, "", 0), "").follow("") }()
	for deadline := time.Now().Add(10 * time.Second); !readLive(t, "server").Answering; time.Sleep(100 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the machine was never marked as answering")
		}
	}
	cancel()
	<-stopped
	if live := readLive(t, "server"); live.Answering || live.Heard.IsZero() {
		t.Errorf("after the connection dropped the live file says %+v", live)
	}
}

func TestStoppingTheWatcherMarksEveryMachineAsNotAnswering(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
	cmd := exec.Command(os.Args[0])
	cmd.Env = append(os.Environ(), "MAGI_TEST_RUN=watch")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	names := []string{f.machine().Name(), "server"}
	for deadline := time.Now().Add(15 * time.Second); !readLive(t, names[0]).Answering || !readLive(t, names[1]).Answering; time.Sleep(100 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the machines were never marked as answering")
		}
	}
	loops := descendants(cmd.Process.Pid)
	t.Cleanup(func() {
		for _, pid := range loops {
			syscall.Kill(pid, syscall.SIGKILL)
		}
	})
	cmd.Process.Signal(syscall.SIGTERM)
	cmd.Wait()
	for _, name := range names {
		if readLive(t, name).Answering {
			t.Errorf("after the watcher stopped, %s still reads as answering", name)
		}
	}
}

func TestANewWatchLoopStopsOrphanedLoopsForItsMachine(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	r := f.remoteHost()
	line := func(name string) string {
		args := r.Command(`: "$HOME/.magi/watchers"; while :; do sleep 1; done`, name, "claude").Args
		return args[len(args)-1]
	}
	orphan := func(name string) int {
		out, err := exec.Command("sh", "-c", `sh -c "$1" >/dev/null 2>&1 & echo $!`, "sh", line(name)).Output()
		if err != nil {
			t.Fatal(err)
		}
		pid, err := strconv.Atoi(strings.TrimSpace(string(out)))
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { syscall.Kill(pid, syscall.SIGKILL) })
		return pid
	}
	old, other := orphan("laptop"), orphan("other")
	connected := exec.Command("sh", "-c", line("laptop"))
	if err := connected.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { connected.Process.Kill(); connected.Wait() })
	cmd := r.Command(watchScript, "laptop", "claude")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	running := func(pid int) bool {
		if stat, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid)); err == nil {
			return !strings.Contains(string(stat), ") Z ")
		}
		return syscall.Kill(pid, 0) == nil
	}
	for deadline := time.Now().Add(10 * time.Second); running(old); time.Sleep(100 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the orphaned loop for this machine is still running")
		}
	}
	if !running(other) || !running(connected.Process.Pid) {
		t.Error("the new loop stopped a loop for another machine or one that still has its connection")
	}
}

func TestTheWatcherRestartsForAnUpdateOnlyOnceNoSessionIsBeingTakenBack(t *testing.T) {
	newFixture(t)
	every := heartbeatEvery
	heartbeatEvery = 10 * time.Millisecond
	t.Cleanup(func() { heartbeatEvery = every })
	link, err := magiLink()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(link), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"magi-1", "magi-2"} {
		if err := os.WriteFile(filepath.Join(filepath.Dir(link), name), nil, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink("magi-1", link); err != nil {
		t.Fatal(err)
	}
	taking.RLock()
	restarted := make(chan struct{})
	go heartbeat(log.New(io.Discard, "", 0), func() {
		taking.Unlock()
		close(restarted)
		runtime.Goexit()
	})
	time.Sleep(50 * time.Millisecond)
	if err := os.Remove(link); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("magi-2", link); err != nil {
		t.Fatal(err)
	}
	select {
	case <-restarted:
		t.Fatal("the watcher restarted while it was taking a session back")
	case <-time.After(200 * time.Millisecond):
	}
	taking.RUnlock()
	select {
	case <-restarted:
	case <-time.After(5 * time.Second):
		t.Fatal("the watcher never restarted for the update")
	}
}

func TestTheWatcherRestartsWhenAMachineIsAdded(t *testing.T) {
	f := newFixture(t)
	every := heartbeatEvery
	heartbeatEvery = 10 * time.Millisecond
	t.Cleanup(func() { heartbeatEvery = every })
	f.config("local_roots = [\"/tmp\"]\n")
	restarted := make(chan struct{})
	go heartbeat(log.New(io.Discard, "", 0), func() {
		taking.Unlock()
		close(restarted)
		runtime.Goexit()
	})
	time.Sleep(50 * time.Millisecond)
	f.config("local_roots = [\"/tmp\"]\n[hosts.server]\nssh = \"server\"\n")
	select {
	case <-restarted:
	case <-time.After(5 * time.Second):
		t.Fatal("the watcher kept running without the new machine")
	}
}
