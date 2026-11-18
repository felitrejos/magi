package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestATmuxGroupNamedOutsideASCIIKeepsItsTabsInOneSession(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	os.Unsetenv("TMUX")
	tm := Tmux{remote: f.remoteHost()}
	first, err := tm.Start("grp·é", "api · claude", f.remote, []string{"sleep", "60"}, false)
	if err != nil {
		t.Fatal(err)
	}
	second, err := tm.Start("grp·é", "web · claude", f.remote, []string{"sleep", "60"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if !first.SessionCreated || second.SessionCreated || second.Session != first.Session {
		t.Errorf("first = %+v, second = %+v, want the second tab in the first one's session", first, second)
	}
}

func TestATmuxPaneGetsTheMachinesLocaleRatherThanMagis(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	t.Setenv("LC_ALL", "en_US.UTF-8")
	tm := Tmux{remote: f.remoteHost()}
	dump := filepath.Join(f.home, "env")
	if _, err := tm.Start("acme", "api · claude", f.remote, []string{"sh", "-c", `env > "$1.tmp" && mv "$1.tmp" "$1" && exec sleep 60`, "sh", dump}, false); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, dump, 5*time.Second) {
		t.Fatal("the pane never ran")
	}
	if env := strings.Split(f.read(f.home, "env"), "\n"); !slices.Contains(env, "LC_ALL=en_US.UTF-8") {
		t.Errorf("the pane's LC_ALL isn't the machine's own:\n%s", strings.Join(env, "\n"))
	}
	if env, _ := exec.Command("tmux", "show-environment", "-g", "LC_ALL").CombinedOutput(); strings.TrimSpace(string(env)) != "LC_ALL=en_US.UTF-8" {
		t.Errorf("tmux's global LC_ALL = %s", env)
	}
}

func TestRemoteCommandsGiveUpOnADeadConnection(t *testing.T) {
	f := newFixture(t)
	args := strings.Join(f.remoteHost().Command("true").Args, " ")
	for _, want := range []string{"-o ServerAliveInterval=15", "-o ServerAliveCountMax=3"} {
		if !strings.Contains(args, want) {
			t.Errorf("missing %q in %s", want, args)
		}
	}
}
