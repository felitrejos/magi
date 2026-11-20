package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/store"
)

const fakePS = `#!/bin/sh
printf '%s 1 0:02.00 102400\n99999 %s 0:01.00 51200\n' "$MAGI_TEST_PID" "$MAGI_TEST_PID"
`

func readLive(t *testing.T, name string) liveMachine {
	t.Helper()
	path, err := store.Dir(filepath.Join("live", name+".json"))
	if err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	var live liveMachine
	json.Unmarshal(data, &live)
	return live
}

func TestWatchStreamsWhatChanges(t *testing.T) {
	f := newFixture(t)
	server := f.serverHome()
	pid := startFakeClaude(t, "60").Process.Pid
	t.Setenv("MAGI_TEST_PID", fmt.Sprint(pid))
	f.tool("ps", fakePS)
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	busySince := time.Now().Add(-time.Minute).Truncate(time.Millisecond)
	session := func(status string, at time.Time) {
		f.write(filepath.Join(server, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"%s","cwd":"/srv/api","name":"api","status":"%s","statusUpdatedAt":%d}`, pid, id, status, at.UnixMilli()))
	}
	session("busy", busySince)
	r := f.remoteHost()
	w := newWatcher("server", r, log.New(io.Discard, "", 0))
	cmd := r.Command(watchScript, "", "claude", "")
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
	if tree := fmt.Sprintf(" %d:3.00:150", pid); !strings.HasSuffix(heard[len(heard)-1], tree) {
		t.Errorf("stats line = %q, want the session's tree summed to %q", heard[len(heard)-1], tree)
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
		if !strings.HasPrefix(line, "stats") {
			sessionLines = append(sessionLines, line)
		}
	}
	if len(sessionLines) != 4 || !strings.Contains(sessionLines[0], `"busy"`) || !strings.Contains(sessionLines[2], `"idle"`) {
		t.Errorf("session lines = %q, want one snapshot when busy and one when idle", sessionLines)
	}
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
	go func() { stopped <- newWatcher("server", r, log.New(io.Discard, "", 0)).follow("") }()
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
