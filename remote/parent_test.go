package main

import (
	"path/filepath"
	"strconv"
	"testing"
)

func TestWorkSentFromAnUnnamedOrUnknownSessionFallsBack(t *testing.T) {
	f := newFixture(t)
	pid := startFakeClaude(t, "60").Process.Pid
	f.write(filepath.Join(f.home, ".claude", "sessions"), strconv.Itoa(pid)+".json", `{"pid":`+strconv.Itoa(pid)+`,"sessionId":"07f37932"}`)
	if unnamed, err := delegatorLabel(pid, "stampr"); err != nil || unnamed != machineLabel()+" ← stampr" {
		t.Fatal(unnamed, err)
	}
	for _, pid := range []int{0, startFakeClaude(t, "60").Process.Pid} {
		if none, err := delegatorLabel(pid, "stampr"); none != "" || err != nil {
			t.Fatal(pid, none, err)
		}
	}
}
