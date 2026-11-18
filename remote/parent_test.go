package main

import (
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func TestWorkSentElsewhereIsNamedAfterAndTaggedWithTheSessionThatSentIt(t *testing.T) {
	f := newFixture(t)
	session := func(body string) int {
		pid := startFakeClaude(t, "60").Process.Pid
		f.write(filepath.Join(f.home, ".claude", "sessions"), strconv.Itoa(pid)+".json", `{"pid":`+strconv.Itoa(pid)+`,`+body+`}`)
		return pid
	}
	named, err := delegatingSession(caller{PID: session(`"sessionId":"9fb6fe92","name":"fix login"`)}, "stampr")
	if err != nil || named == nil {
		t.Fatal(named, err)
	}
	if label := named.tabLabel(); !strings.HasSuffix(label, " ← fix login") || label == " ← fix login" {
		t.Fatalf("%q", label)
	}
	command := withParent(named.ID, []string{"sh", "-c", "x"})
	if strings.Join(command, " ") != "env MAGI_PARENT=9fb6fe92 sh -c x" {
		t.Fatal(command)
	}
	unnamed, err := delegatingSession(caller{PID: session(`"sessionId":"07f37932"`)}, "stampr")
	if err != nil || unnamed.Name != "stampr" {
		t.Fatal(unnamed, err)
	}
	for _, pid := range []int{0, startFakeClaude(t, "60").Process.Pid} {
		if none, err := delegatingSession(caller{PID: pid}, "stampr"); none != nil || err != nil {
			t.Fatal(pid, none, err)
		}
	}
}
