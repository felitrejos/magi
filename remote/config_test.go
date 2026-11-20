package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/felitrejos/magi/remote/internal/sh"
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

func TestAProjectsGroupIsItsFolderUnderARootOrElseItsParent(t *testing.T) {
	for dir, want := range map[string]string{
		"/home/me/developer/acme/api-server": "acme",
		"/home/me/developer/notes":           "notes",
		"/srv/other/thing":                   "other",
	} {
		if got := groupOf(dir, []string{"~/Missing", "~/developer"}, "/home/me"); got != want {
			t.Errorf("groupOf(%s) = %q, want %q", dir, got, want)
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
		if got := sh.ExpandHome(in, "/h"); got != want {
			t.Errorf("ExpandHome(%q) = %q, want %q", in, got, want)
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

[hosts."odd.name"]
ssh = "odd"
claude = "claude"
roots = ["~/d"]
`)
	cfg, path, err := loadConfig()
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(cfg.Order, "|"); got != "box|lab|odd.name" {
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
	if got := strings.Join(again.Order, "|"); got != "box|lab|odd.name|new" || again.Hosts["lab"].SSH != "lab" || again.Hosts["new"].MinFreeMemory != "2G" {
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

func TestHostNamesThatArentFileNamesAndSSHOptionsAreRefused(t *testing.T) {
	f := newFixture(t)
	f.config("[hosts.\"../up\"]\nssh = \"box\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
	if text, failed := f.call("machines", nil); !failed || !strings.Contains(text, "isn't a machine name") {
		t.Errorf("machines with a host called ../up:\n%s", text)
	}
	f.config("")
	if text, failed := f.call("setup_machine", map[string]any{"host": "server", "ssh": "-oProxyCommand=true"}); !failed || !strings.Contains(text, "ssh destination") {
		t.Errorf("setup_machine with an ssh option:\n%s", text)
	}
}

func TestAHandWrittenConfigGetsTheDefaultExcludeAndSecrets(t *testing.T) {
	f := newFixture(t)
	f.config("[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n\n[hosts.own]\nssh = \"own\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\nsecrets = [\"*.pem\"]\n")
	host, err := loadHost("server")
	if err != nil || strings.Join(host.Exclude, " ") != "node_modules .venv dist build target" || strings.Join(host.Secrets, " ") != ".env .env.*" {
		t.Errorf("server = %+v, %v", host, err)
	}
	if own, err := loadHost("own"); err != nil || strings.Join(own.Secrets, " ") != "*.pem" {
		t.Errorf("own = %+v, %v", own, err)
	}
}
