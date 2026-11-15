package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

const forwardSession = "0f0f0f0f-1111-2222-3333-444444444444"

func TestProjectPortsAreValidatedAndCanStandAlone(t *testing.T) {
	p, err := parseProject([]byte("ports = [3000, 5173]\n"))
	if err != nil || len(p.Ports) != 2 {
		t.Fatalf("p = %+v, err = %v", p, err)
	}
	for _, bad := range []string{"ports = [0]", "ports = [70000]", "ports = [3000, 3000]"} {
		if _, err := parseProject([]byte(bad)); err == nil {
			t.Errorf("%q was accepted", bad)
		}
	}
}

func TestForwardArgsBindOnlyThisMachineAndUseTheirOwnConnection(t *testing.T) {
	args := strings.Join(forwardArgs(forwardRecord{SSH: "homelab", Ports: []int{3000, 8080}}), " ")
	for _, want := range []string{"-N", "ExitOnForwardFailure=yes", "ControlPath=none", "-L 127.0.0.1:3000:localhost:3000", "-L 127.0.0.1:8080:localhost:8080"} {
		if !strings.Contains(args, want) {
			t.Errorf("missing %q in %s", want, args)
		}
	}
	if !strings.HasSuffix(args, " -- homelab") {
		t.Errorf("the destination isn't last: %s", args)
	}
}

func forwardHome(t *testing.T) string {
	t.Helper()
	t.Setenv("HOME", t.TempDir())
	path, err := forwardPath(forwardSession)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	return path
}

func alive(pid int) bool {
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
	lock, err := holdLock(path)
	if err != nil {
		t.Fatal(err)
	}
	defer lock.Close()
	if err := writeForward(path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{3000}, PID: runner.Process.Pid}); err != nil {
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
	if err := closeForwardsCommand(t, forwardSession[:8]); err != nil {
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

func closeForwardsCommand(t *testing.T, id string) error {
	t.Helper()
	var out strings.Builder
	return forwardsCommand([]string{"close", id}, &out)
}

func TestARecordFromADeadRunnerNeverSignalsWhateverReusedItsPID(t *testing.T) {
	path := forwardHome(t)
	stranger := exec.Command("sleep", "30")
	stranger.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := stranger.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stranger.Process.Kill() })
	if err := writeForward(path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{3000}, PID: stranger.Process.Pid, State: forwardConnected}); err != nil {
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
	bin := t.TempDir()
	if err := os.WriteFile(filepath.Join(bin, "ssh"), []byte("#!/bin/sh\nsleep 0.6\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+":"+os.Getenv("PATH"))
	forwardSettle, forwardRetry = 150*time.Millisecond, 50*time.Millisecond
	t.Cleanup(func() { forwardSettle, forwardRetry = 3*time.Second, 2*time.Second })
	if err := writeForward(path, forwardRecord{Session: forwardSession, Host: "homelab", SSH: "homelab", Ports: []int{3000}, State: forwardWaiting}); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- runForwards(forwardSession) }()
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
