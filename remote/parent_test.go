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
	unnamed, err := delegatingSession(pid, "stampr")
	if err != nil || unnamed == nil || unnamed.Name != "stampr" {
		t.Fatal(unnamed, err)
	}
	for _, pid := range []int{0, startFakeClaude(t, "60").Process.Pid} {
		if none, err := delegatingSession(pid, "stampr"); none != nil || err != nil {
			t.Fatal(pid, none, err)
		}
	}
}
