package main

import (
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const fakeClaudeToSetUp = "#!/bin/sh\n" + fakeAgents + `case "$1 $2" in
  "auth status") echo '{"loggedIn":true,"authMethod":"claude.ai"}'; exit ;;
  "plugin list")
    if [ -d "$HOME/magi-plugin" ]; then printf '[{"id":"magi@magi","version":"1.0.0","enabled":true,"installPath":"%s"}]\n' "$HOME/magi-plugin"; else echo '[]'; fi
    exit ;;
  "plugin marketplace") echo "$*" >> "$HOME/claude-calls"; echo '[]'; exit ;;
  "plugin update") echo "$*" >> "$HOME/claude-calls"; exit ;;
  "plugin install")
    echo "$*" >> "$HOME/claude-calls"
    mkdir -p "$HOME/magi-plugin/bin"
    printf '#!/bin/sh\necho magi 9.9.9\n' > "$HOME/magi-plugin/bin/magi"
    chmod +x "$HOME/magi-plugin/bin/magi"
    exit ;;
esac
id=0f0f0f0f-1111-2222-3333-121212121212
transcript=$HOME/.claude/projects/$(printf '%s' "$PWD" | sed 's/[^A-Za-z0-9]/-/g')/$id.jsonl
if [ "$1" = -p ]; then
  case $* in
    *--resume*) printf '{"session_id":"%s","result":"%s"}\n' "$id" "$(grep -o 'word [A-Z]*' "$transcript" | head -n 1 | cut -c 6-)" ;;
    *)
      mkdir -p "${transcript%/*}"
      printf '{"cwd":"%s","type":"user","message":{"content":"%s"}}\n' "$PWD" "$6" > "$transcript"
      printf '{"session_id":"%s","result":"ok"}\n' "$id" ;;
  esac
  exit
fi
` + fakeStart + "exec sleep 60\n"

func TestSetupMachineSetsUpTheOtherMachineAndProvesARoundTrip(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	server := f.serverHome()
	f.tool("claude", fakeClaudeToSetUp)
	if err := os.MkdirAll(filepath.Join(server, "code"), 0o755); err != nil {
		t.Fatal(err)
	}
	setup := map[string]any{"host": "server", "ssh": "fakehost", "local_roots": []string{"~/laptop"}}

	for down, want := range map[string]string{"": "Network is unreachable", "Host key verification failed.\n": "doesn't know fakehost's host key"} {
		f.write(f.home, "ssh-down", down)
		if text, failed := f.call("setup_machine", setup); failed || !strings.Contains(text, want) || !strings.Contains(text, "setup_machine again") {
			t.Fatalf("with ssh failing with %q:\n%s", down, text)
		}
	}
	if err := os.Remove(filepath.Join(f.home, "ssh-down")); err != nil {
		t.Fatal(err)
	}
	text, failed := f.call("setup_machine", setup)
	for _, want := range []string{"Installed the magi plugin on fakehost", "✓ the magi plugin on server runs magi 9.9.9", "✓ logged in to claude.ai on server", "✓ tmux on server", "magi works with server"} {
		if failed || !strings.Contains(text, want) {
			t.Errorf("setup_machine lacks %q:\n%s", want, text)
		}
	}
	if calls := f.read(server, "claude-calls"); !strings.Contains(calls, "plugin marketplace add felitrejos/magi") || !strings.Contains(calls, "plugin install magi@magi") {
		t.Errorf("claude ran on the server with %q", calls)
	}
	started := f.read(f.home, "service-calls")
	if !strings.Contains(started, watchService) {
		t.Errorf("the watcher wasn't started here: %q", started)
	}
	newer := version
	version = "1.1.0"
	t.Cleanup(func() { version = newer })
	if text, failed := f.call("setup_machine", setup); failed || !strings.Contains(text, "magi watch already runs here") || !strings.Contains(text, "Updated the magi plugin on fakehost") {
		t.Errorf("checking again with a newer plugin here:\n%s", text)
	}
	if calls := f.read(server, "claude-calls"); !strings.Contains(calls, "plugin marketplace update magi") || !strings.Contains(calls, "plugin update magi@magi") {
		t.Errorf("the older plugin on the server wasn't updated: %q", calls)
	}
	if again := f.read(f.home, "service-calls"); strings.Count(again, "bootstrap")+strings.Count(again, "restart") != 1 {
		t.Errorf("checking again restarted the watcher: %q", again)
	}
	service, err := watchServicePath()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(service); err != nil {
		t.Fatal(err)
	}
	f.tool("launchctl", "#!/bin/sh\necho no >&2\nexit 1\n")
	f.tool("systemctl", "#!/bin/sh\necho no >&2\nexit 1\n")
	if text, failed := f.call("setup_machine", setup); failed || !strings.Contains(text, "✗ magi watch couldn't start here") || !strings.Contains(text, "✓ tmux on server") {
		t.Errorf("with the watcher failing to start:\n%s", text)
	}
	h, err := loadHost("server")
	if err != nil || h.SSH != "fakehost" || strings.Join(h.Roots, ",") != "~/code" || strings.Join(h.LocalRoots, ",") != "~/laptop" {
		t.Errorf("host = %+v, %v", h, err)
	}
}

func TestTryRemovesOnlyItsOwnProject(t *testing.T) {
	f := newFixture(t)
	project := "magi-try-0123abcd"
	keep := filepath.Join(f.home, "laptop", "magi-try-keep")
	f.write(keep, "notes.txt", "mine")
	for _, dir := range []string{
		filepath.Join(f.home, "laptop", project),
		filepath.Join(f.home, "developer", project, project),
		filepath.Join(f.home, ".claude", "projects", "-home-laptop-"+project),
	} {
		f.write(dir, "notes.txt", "test")
	}
	host := Host{SSH: "fakehost", Claude: "claude", Roots: []string{"~/developer"}}
	if err := tryCleanup(f.machine(), []string{"~/laptop"}, host, "magi-try-keep", ""); err == nil {
		t.Fatal("removed a folder that isn't a magi try project")
	}
	if err := tryCleanup(f.machine(), []string{"~/laptop"}, host, project, ""); err != nil {
		t.Fatal(err)
	}
	for _, dir := range []string{filepath.Join(f.home, "laptop", project), filepath.Join(f.home, "developer", project), filepath.Join(f.home, ".claude", "projects", "-home-laptop-"+project)} {
		if _, err := os.Stat(dir); !os.IsNotExist(err) {
			t.Errorf("%s is still there", dir)
		}
	}
	if f.read(keep, "notes.txt") != "mine" {
		t.Error("another folder was touched")
	}
}

func TestUninstallStopsTheWatcherAndRemovesTheBinariesOnly(t *testing.T) {
	f := newFixture(t)
	f.config("local_roots = [\"~/laptop\"]\n")
	if err := installWatch(io.Discard); err != nil {
		t.Fatal(err)
	}
	f.write(filepath.Join(f.home, ".magi", "bin"), "magi-1.0.0", "binary")
	f.write(filepath.Join(f.home, ".magi"), "handoffs.jsonl", "{}\n")
	if text, failed := f.call("uninstall", nil); failed || !strings.Contains(text, "/plugin") {
		t.Fatalf("uninstall:\n%s", text)
	}
	service, err := watchServicePath()
	if err != nil {
		t.Fatal(err)
	}
	for _, gone := range []string{service, filepath.Join(f.home, ".magi", "bin")} {
		if _, err := os.Stat(gone); !os.IsNotExist(err) {
			t.Errorf("%s is still there", gone)
		}
	}
	if f.read(filepath.Join(f.home, ".magi"), "handoffs.jsonl") != "{}\n" || f.read(filepath.Join(f.home, ".config", "magi"), "config.toml") == "<missing>" {
		t.Error("uninstall removed magi's config or state")
	}
	if calls := f.read(f.home, "service-calls"); !strings.Contains(calls, "bootout") && !strings.Contains(calls, "disable --now") {
		t.Errorf("the watcher wasn't stopped: %q", calls)
	}
}
