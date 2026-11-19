package main

import (
	"context"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestHostRejectsRelativeRoots(t *testing.T) {
	h := Host{SSH: "s", Claude: "c", Roots: []string{"developer"}}
	if err := h.validate("server"); err == nil {
		t.Fatal("a relative root was accepted")
	}
	h.Roots = []string{"~/developer", "/srv/code"}
	if err := h.validate("server"); err != nil {
		t.Fatal(err)
	}
	h.Exclude = []string{"build/cache"}
	if err := h.validate("server"); err == nil {
		t.Fatal("an exclude with a slash was accepted, though find and tar would read it differently")
	}
}

func TestGroupsMirrorTheLaptopAndKeepTheServersLayout(t *testing.T) {
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	for _, dir := range []string{"Projects/acme/api", "Projects/notes", "elsewhere/tool"} {
		if err := os.MkdirAll(filepath.Join(home, dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	roots := []string{"~/Missing", "~/Projects"}
	for dir, want := range map[string]string{
		"Projects/acme/api": "acme",
		"Projects/notes":    "notes",
		"elsewhere/tool":    "tool",
		"Projects":          "Projects",
	} {
		if got := localGroup(filepath.Join(home, dir), roots); got != want {
			t.Errorf("localGroup(%s) = %q; want %q", dir, got, want)
		}
	}
	for clone, want := range map[string]string{
		"/home/me/developer/acme/api-server": "acme",
		"/home/me/developer/notes":           "notes",
		"/srv/other/thing":                   "other",
	} {
		if got := serverGroup(clone, []string{"~/developer"}, "/home/me"); got != want {
			t.Errorf("serverGroup(%s) = %q, want %q", clone, got, want)
		}
	}
}

func TestExpandHomeOnlyExpandsALeadingTilde(t *testing.T) {
	for in, want := range map[string]string{
		"~/.local/bin/claude": "/h/.local/bin/claude",
		"~":                   "/h",
		"/opt/claude~1/bin":   "/opt/claude~1/bin",
		"~other/bin":          "~other/bin",
	} {
		if got := expandHome(in, "/h"); got != want {
			t.Errorf("expandHome(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestDoctorFailsVersionsMagiHasntTested(t *testing.T) {
	r := testedRange{Min: "2.1.300", Max: "2.1.327"}
	for text, want := range map[string]string{
		"2.1.1 (Claude Code)":   "✗",
		"2.1.327 (Claude Code)": "✓",
		"9.0.0 (Claude Code)":   "✗",
	} {
		var out strings.Builder
		c := &checks{out: &out}
		c.tested(r, "Claude Code", "here", text)
		if !strings.HasPrefix(strings.TrimSpace(out.String()), want) {
			t.Errorf("%s gave %q, want %s", text, out.String(), want)
		}
	}
}

func TestTheSameRootListedTwiceCountsOnce(t *testing.T) {
	f := newFixture(t)
	alias := filepath.Join(f.home, "dev-alias")
	if err := os.Symlink(filepath.Join(f.home, "developer"), alias); err != nil {
		t.Fatal(err)
	}
	clones, err := listClones(f.remoteHost(), []string{"~/developer", "~/dev-alias"})
	if err != nil {
		t.Fatal(err)
	}
	if got, err := matchClone(clones, f.workspace(f.local), "server"); err != nil || got != f.remote {
		t.Fatalf("matchClone = %q, %v; want one clone", got, err)
	}
}

func TestInitWritesTheConfigAndKeepsAnswersWhenRunAgain(t *testing.T) {
	f := newFixture(t)
	f.tool("tailscale", "#!/bin/sh\nexit 1\n")
	claude := f.tool("claude", fakeClaudeCLI)
	codex := f.tool("codex", "#!/bin/sh\n")
	answers := strings.Join([]string{"~/laptop", "", "server", "fakehost", claude, codex, "~/developer", ""}, "\n") + "\n"
	initConfig(strings.NewReader(answers), io.Discard)
	h, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	if h.SSH != "fakehost" || h.Claude != claude || h.Codex != codex || strings.Join(h.Roots, ",") != "~/developer" || strings.Join(h.Secrets, ",") != ".env,.env.*" || !slices.Contains(h.Exclude, "node_modules") {
		t.Fatalf("host = %+v", h)
	}
	initConfig(strings.NewReader("server\n"+strings.Repeat("\n", 6)), io.Discard)
	again, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	if again.Claude != h.Claude || again.Codex != h.Codex || strings.Join(again.Roots, ",") != "~/developer" {
		t.Errorf("running init again and pressing Enter changed the host: %+v", again)
	}
}

func TestDoctorChecksTheServer(t *testing.T) {
	f := newFixture(t)
	claude := f.tool("claude", fakeClaudeCLI)
	var out strings.Builder
	c := &checks{out: &out}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	doctorHost(ctx, c, testedVersions{Claude: testedRange{Min: "2.1.300", Max: "2.1.327"}}, "server", Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer", "~/missing"}})
	report := out.String()
	for _, want := range []string{"✓ ssh fakehost works", "✓ git 2.", "✓ Claude Code 2.1.325 on server", "✓ logged in to claude.ai on server", "✓ root ~/developer exists", "✗ root ~/missing exists"} {
		if !strings.Contains(report, want) {
			t.Errorf("doctor report lacks %q:\n%s", want, report)
		}
	}
}
