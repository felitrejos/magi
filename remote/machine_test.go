package main

import (
	"context"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func TestRemoteErrorsShowTheShellsMessageWithoutTheScript(t *testing.T) {
	f := newFixture(t)
	_, err := sh.Run(f.remoteHost(), `cd -- "$1"`, "/nonexistent-folder")
	if err == nil || strings.Contains(err.Error(), "set --") || len(err.Error()) > 200 || !strings.Contains(err.Error(), "/nonexistent-folder") {
		t.Errorf("err = %q", err)
	}
}

func TestRemoteScriptsReceiveEveryByteWhateverTheLoginShell(t *testing.T) {
	var tricky strings.Builder
	for b := 1; b < 256; b++ {
		tricky.WriteByte(byte(b))
	}
	args := []string{tricky.String(), `it's "$HOME" ; rm -rf / $(x) %s %%`, "", "-n", "line one\nline two"}
	for _, shell := range []string{"sh", "tcsh", "fish"} {
		t.Run(shell, func(t *testing.T) {
			path, err := exec.LookPath(shell)
			if err != nil {
				t.Skip(shell + " isn't installed")
			}
			f := newFixture(t)
			t.Setenv("MAGI_TEST_LOGIN_SHELL", path)
			out, err := sh.Run(f.remoteHost(), `for a; do printf '%s\0' "$a"; done`, args...)
			if err != nil {
				t.Fatal(err)
			}
			got := strings.Split(strings.TrimSuffix(string(out), "\x00"), "\x00")
			if len(got) != len(args) {
				t.Fatalf("got %d arguments, want %d", len(got), len(args))
			}
			for i := range args {
				if got[i] != args[i] {
					t.Errorf("argument %d = %q, want %q", i, got[i], args[i])
				}
			}
		})
	}
}

func TestAMachineThatStopsAnsweringSaysSo(t *testing.T) {
	f := newFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
	defer cancel()
	hung, err := newRemote(ctx, Host{SSH: "fakehost"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := sh.Run(hung, "sleep 5"); err == nil || err.Error() != "fakehost stopped answering" {
		t.Errorf("past the deadline err = %v", err)
	}
	r := f.remoteHost()
	f.tool("ssh", "#!/bin/sh\nkill -KILL $$\n")
	if _, err := sh.Run(r, "true"); err == nil || err.Error() != "fakehost stopped answering" {
		t.Errorf("with ssh killed err = %v", err)
	}
	if err := sh.Transfer(f.machine(), f.local, sh.Archive{Paths: []string{"kept.txt"}}, r, f.remote); err == nil || err.Error() != "fakehost stopped answering" {
		t.Errorf("copying with ssh killed err = %v", err)
	}
	if _, err := newRemote(context.Background(), Host{SSH: "fakehost"}); err == nil || err.Error() != "fakehost stopped answering" {
		t.Errorf("connecting with ssh killed err = %v", err)
	}
}
