package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestWorkSentElsewhereIsNamedAfterAndTaggedWithTheSessionThatSentIt(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	sessions := filepath.Join(home, ".claude", "sessions")
	if err := os.MkdirAll(sessions, 0o700); err != nil {
		t.Fatal(err)
	}
	write := func(pid, body string) {
		if err := os.WriteFile(filepath.Join(sessions, pid+".json"), []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("41", `{"pid":41,"sessionId":"9fb6fe92","name":"fix login"}`)
	write("42", `{"pid":42,"sessionId":"07f37932"}`)

	named, err := delegatingSession(caller{PID: 41}, "stampr")
	if err != nil || named == nil {
		t.Fatal(named, err)
	}
	if label := named.tabLabel(); !strings.HasSuffix(label, " ← fix login") || label == " ← fix login" {
		t.Fatalf("%q", label)
	}
	command := named.wrap([]string{"sh", "-c", "x"})
	if strings.Join(command, " ") != "env CIELAGO_PARENT=9fb6fe92 sh -c x" {
		t.Fatal(command)
	}
	unnamed, err := delegatingSession(caller{PID: 42}, "stampr")
	if err != nil || unnamed.Name != "stampr" {
		t.Fatal(unnamed, err)
	}
	for _, pid := range []int{0, 43} {
		if none, err := delegatingSession(caller{PID: pid}, "stampr"); none != nil || err != nil {
			t.Fatal(pid, none, err)
		}
	}
}
