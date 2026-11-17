package main

import (
	"os/exec"
	"strings"
	"testing"
)

func TestRemoteErrorsShowTheShellsMessageWithoutTheScript(t *testing.T) {
	f := newFixture(t)
	_, err := sh(f.remoteHost(), `cd -- "$1"`, "/nonexistent-folder")
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
			t.Setenv("CIELAGO_TEST_LOGIN_SHELL", path)
			out, err := sh(f.remoteHost(), `for a; do printf '%s\0' "$a"; done`, args...)
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

func TestQuoteSurvivesTheShell(t *testing.T) {
	tricky := `it's "$HOME" ; rm -rf / $(x)`
	out, err := exec.Command("sh", "-c", "printf %s "+quote(tricky)).Output()
	if err != nil {
		t.Fatal(err)
	}
	if string(out) != tricky {
		t.Errorf("got %q, want %q", out, tricky)
	}
}
