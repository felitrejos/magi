package main

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/store"
)

const forwardSession = "0f0f0f0f-1111-2222-3333-444444444444"

func forwardHome(t *testing.T) string {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	path, err := store.SessionPath("forwards", forwardSession, ".json")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	return path
}

func alive(pid int) bool {
	if stat, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid)); err == nil {
		fields := strings.Fields(string(stat))
		return len(fields) > 2 && fields[2] != "Z"
	}
	return syscall.Kill(pid, 0) == nil
}

func TestClosingAForwardStopsItsRunnerAndEverythingItStarted(t *testing.T) {
	path := forwardHome(t)
	childFile := filepath.Join(t.TempDir(), "child")
	runner := exec.Command("sh", "-c", `sleep 30 & echo $! > "$1"; wait`, "sh", childFile)
	runner.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := runner.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { syscall.Kill(-runner.Process.Pid, syscall.SIGKILL) })
	unlock, err := holdLock(path)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	if err := store.WriteJSON(path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{3000}, PID: runner.Process.Pid}); err != nil {
		t.Fatal(err)
	}
	var child int
	for deadline := time.Now().Add(5 * time.Second); child == 0 && time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		data, _ := os.ReadFile(childFile)
		child, _ = strconv.Atoi(strings.TrimSpace(string(data)))
	}
	if child == 0 {
		t.Fatal("the runner's child never started")
	}
	if err := closeForwards(forwardSession); err != nil {
		t.Fatal(err)
	}
	runner.Wait()
	for deadline := time.Now().Add(5 * time.Second); alive(child) && time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
	}
	if alive(child) {
		t.Error("the runner's child outlived it")
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Errorf("the record is still there: %v", err)
	}
}

func TestARecordFromADeadRunnerNeverSignalsWhateverReusedItsPID(t *testing.T) {
	path := forwardHome(t)
	stranger := exec.Command("sleep", "30")
	stranger.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := stranger.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stranger.Process.Kill() })
	if err := store.WriteJSON(path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{3000}, PID: stranger.Process.Pid, State: forwardConnected}); err != nil {
		t.Fatal(err)
	}
	records, err := listForwards()
	if err != nil || len(records) != 1 || records[0].State != "stopped" {
		t.Fatalf("records = %+v, %v", records, err)
	}
	if err := closeForwards(forwardSession); err != nil {
		t.Fatal(err)
	}
	time.Sleep(100 * time.Millisecond)
	if !alive(stranger.Process.Pid) {
		t.Error("an unrelated process with the old PID was killed")
	}
	if err := closeForwards(forwardSession); err != nil {
		t.Errorf("closing twice: %v", err)
	}
}

func TestTheRunnerSaysWhenItsTunnelIsUpAndRetriesUntilItIsClosed(t *testing.T) {
	path := forwardHome(t)
	fakeTunnel(t, "ssh", "sleep 0.6\n")
	listener, port := listen(t)
	defer listener.Close()
	done := startRunner(t, path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{port}, State: forwardWaiting})
	seen := map[string]bool{}
	for deadline := time.Now().Add(5 * time.Second); !(seen[forwardConnected] && runnerAlive(path)) && time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		if r, err := readForward(path); err == nil {
			seen[r.State] = true
		}
	}
	if !seen[forwardConnected] || !runnerAlive(path) {
		t.Fatalf("seen = %v, alive = %v", seen, runnerAlive(path))
	}
	if second := runForwards(forwardSession); second != nil {
		t.Errorf("a second runner should step aside: %v", second)
	}
	os.Remove(path)
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the runner kept going after its record was removed")
	}
	if runnerAlive(path) {
		t.Error("the lock is still held after the runner stopped")
	}
}

func listen(t *testing.T) (net.Listener, int) {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	return l, l.Addr().(*net.TCPAddr).Port
}

func freePort(t *testing.T) int {
	t.Helper()
	l, port := listen(t)
	l.Close()
	return port
}

func fakeTunnel(t *testing.T, name, body string) {
	t.Helper()
	bin := t.TempDir()
	if err := os.WriteFile(filepath.Join(bin, name), []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+":"+os.Getenv("PATH"))
}

func startRunner(t *testing.T, path string, record forwardRecord) <-chan error {
	t.Helper()
	if err := store.WriteJSON(path, record); err != nil {
		t.Fatal(err)
	}
	forwardRetry = 50 * time.Millisecond
	done, finished := make(chan error, 1), make(chan struct{})
	go func() {
		done <- runForwards(forwardSession)
		close(finished)
	}()
	t.Cleanup(func() {
		os.Remove(path)
		if pid, err := strconv.Atoi(strings.TrimSpace(readFile(filepath.Join(os.Getenv("HOME"), "tunnel.pid")))); err == nil {
			syscall.Kill(pid, syscall.SIGKILL)
		}
		select {
		case <-finished:
			forwardRetry = 2 * time.Second
		case <-time.After(5 * time.Second):
			t.Error("the runner never stopped")
		}
	})
	return done
}

func readFile(path string) string {
	data, _ := os.ReadFile(path)
	return string(data)
}

func stateFor(path string, within time.Duration, want string) bool {
	for deadline := time.Now().Add(within); time.Now().Before(deadline); time.Sleep(20 * time.Millisecond) {
		if r, err := readForward(path); err == nil && r.State == want {
			return true
		}
	}
	return false
}

const longTunnel = `echo $$ > "$HOME/tunnel.pid"
exec sleep 30
`

func TestAnSSHForwardIsConnectedOnlyOnceEveryPortAccepts(t *testing.T) {
	path := forwardHome(t)
	fakeTunnel(t, "ssh", longTunnel)
	first, firstPort := listen(t)
	defer first.Close()
	secondPort := freePort(t)
	startRunner(t, path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{firstPort, secondPort}, State: forwardWaiting})
	if stateFor(path, 3500*time.Millisecond, forwardConnected) {
		t.Fatal("the forward was connected while ssh still hadn't opened one of its ports")
	}
	second, err := net.Listen("tcp", "127.0.0.1:"+strconv.Itoa(secondPort))
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	if !stateFor(path, 2*time.Second, forwardConnected) {
		t.Error("the forward never counted as connected once every port accepted")
	}
}
