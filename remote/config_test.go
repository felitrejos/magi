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

func TestHostsWrittenWithDottedKeysOrAddedLaterKeepTheirPlace(t *testing.T) {
	f := newFixture(t)
	f.config(`[hosts]
lab.ssh = "lab"
lab.claude = "claude"
lab.roots = ["~/d"]
`)
	if cfg, _, err := loadConfig(); err != nil || strings.Join(cfg.Order, " ") != "lab" {
		t.Errorf("[hosts] with dotted keys: %v, %v", cfg.Order, err)
	}
	f.config(`hosts.box.ssh = "box"
hosts.box.claude = "claude"
hosts.box.roots = ["~/d"]

[hosts.lab]
ssh = "lab"
claude = "claude"
roots = ["~/d"]

[hosts."odd \"name\""]
ssh = "odd"
claude = "claude"
roots = ["~/d"]
`)
	cfg, path, err := loadConfig()
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(cfg.Order, "|"); got != `box|lab|odd "name"` {
		t.Fatalf("order = %s", got)
	}
	cfg.Order = append(cfg.Order, "new")
	cfg.Hosts["new"] = Host{SSH: "new", Claude: "claude", Roots: []string{"~/d"}, MinFreeMemory: "2G"}
	if err := writeConfig(path, cfg); err != nil {
		t.Fatal(err)
	}
	again, _, err := loadConfig()
	if err != nil {
		t.Fatalf("rewritten config doesn't load: %v\n%s", err, f.read(filepath.Dir(path), "config.toml"))
	}
	if got := strings.Join(again.Order, "|"); got != `box|lab|odd "name"|new` || again.Hosts["lab"].SSH != "lab" || again.Hosts["new"].MinFreeMemory != "2G" {
		t.Errorf("after rewriting: order %s, hosts %+v", got, again.Hosts)
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
	codex := f.tool("codex", fakeCodex)
	answers := strings.Join([]string{"~/laptop", "", "server", "fakehost", claude, codex, "~/developer", ""}, "\n") + "\n"
	if err := initConfig(strings.NewReader(answers), io.Discard); err != nil {
		t.Fatal(err)
	}
	h, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	if h.SSH != "fakehost" || h.Claude != claude || h.Codex != codex || strings.Join(h.Roots, ",") != "~/developer" || strings.Join(h.Secrets, ",") != ".env,.env.*" || !slices.Contains(h.Exclude, "node_modules") {
		t.Fatalf("host = %+v", h)
	}
	if err := initConfig(strings.NewReader("server\n"+strings.Repeat("\n", 6)), io.Discard); err != nil {
		t.Fatal(err)
	}
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
	f.tool("magi", "#!/bin/sh\n")
	var out strings.Builder
	c := &checks{out: &out}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	tested, err := parseTested(builtinTested)
	if err != nil {
		t.Fatal(err)
	}
	doctorHost(ctx, c, tested, "server", Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer", "~/missing"}})
	report := out.String()
	for _, want := range []string{"✓ ssh fakehost works", "✓ git 2.", "✓ Claude Code " + tested.Claude.Max + " on server", "✓ logged in to claude.ai on server", "✓ magi is on PATH on server", "✓ the magi plugin is installed and enabled on server", "✓ root ~/developer exists", "✗ root ~/missing exists"} {
		if !strings.Contains(report, want) {
			t.Errorf("doctor report lacks %q:\n%s", want, report)
		}
	}
}

func TestDoctorSaysWhatTheServerIsMissing(t *testing.T) {
	f := newFixture(t)
	claude := f.tool("claude", fakeClaudeCLI)
	t.Setenv("PATH", filepath.Join(f.home, "bin")+":/usr/bin:/bin")
	t.Setenv("MAGI_FAKE_PLUGINS", `[{"id":"magi@magi","enabled":false}]`)
	var out strings.Builder
	c := &checks{out: &out}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	tested, err := parseTested(builtinTested)
	if err != nil {
		t.Fatal(err)
	}
	doctorHost(ctx, c, tested, "server", Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}})
	report := out.String()
	for _, want := range []string{"✗ magi isn't on PATH on server", "install.sh | sh", "✗ the magi plugin is installed but disabled on server", "claude plugin enable magi@magi"} {
		if !strings.Contains(report, want) {
			t.Errorf("doctor report lacks %q:\n%s", want, report)
		}
	}
}
