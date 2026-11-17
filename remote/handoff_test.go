package main

import (
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestAHandoffWorksWhenTheServersLoginShellIsTcsh(t *testing.T) {
	path, err := exec.LookPath("tcsh")
	if err != nil {
		t.Skip("tcsh isn't installed")
	}
	f := newFixture(t)
	t.Setenv("MAGI_TEST_LOGIN_SHELL", path)
	f.messUp(f.local)
	odd := filepath.Join(f.home, "developer", `it's a "repo"`)
	f.git(f.home, "clone", "-q", f.local, odd)
	f.git(odd, "reset", "-q", "--hard", "HEAD~1")
	ws := f.workspace(f.local)
	if err := f.push(f.remoteHost(), ws, Placement{Root: odd}); err != nil {
		t.Fatal(err)
	}
	if got, want := f.git(odd, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("status:\n%s\nwant:\n%s", got, want)
	}
}

func TestTmuxLauncherOpensItsOwnWindowAndCleansUp(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	tm := Tmux{remote: f.remoteHost()}

	first, err := tm.Start("my.group", "api · claude", f.remote, []string{"sleep", "60"}, true)
	if err != nil {
		t.Fatal(err)
	}
	if !first.SessionCreated || !strings.HasPrefix(first.Session, "$") || !strings.HasPrefix(first.Window, "@") || !strings.HasPrefix(first.Pane, "%") {
		t.Fatalf("first tab = %+v", first)
	}
	if env, _ := exec.Command("tmux", "show-environment", "-g", "LC_ALL").CombinedOutput(); strings.Contains(string(env), "LC_ALL=C") {
		t.Errorf("the tmux server started with magi's LC_ALL=C: %s", env)
	}
	second, err := tm.Start("my.group", "web · claude", f.remote, []string{"sleep", "60"}, true)
	if err != nil {
		t.Fatal(err)
	}
	if second.SessionCreated || second.Session != first.Session {
		t.Fatalf("second tab = %+v, want a window in the same session", second)
	}
	windows, _ := exec.Command("tmux", "list-windows", "-t", first.Session, "-F", "#{window_name}").Output()
	if got := strings.Fields(strings.ReplaceAll(string(windows), " · ", "·")); strings.Join(got, ",") != "api·claude,web·claude" {
		t.Errorf("windows = %q", windows)
	}
	if err := tm.Discard(second); err != nil {
		t.Fatal(err)
	}
	if err := tm.Discard(first); err != nil {
		t.Fatal(err)
	}
	if out, _ := exec.Command("tmux", "list-sessions").CombinedOutput(); strings.Contains(string(out), "my.group") {
		t.Errorf("session left behind: %s", out)
	}
}

func TestAHandoffMovesTheSessionAndOpensItInItsOwnTab(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)

	f.messUp(f.local)
	id := "0f0f0f0f-1111-2222-3333-444444444444"
	slug, err := projectSlug(f.local)
	if err != nil {
		t.Fatal(err)
	}
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")

	res, err := Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, Launcher: "tmux", LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}.Run()
	if err != nil {
		t.Fatalf("handoff failed after %s: %v", res.Summary(), err)
	}
	if res.ToDir != f.remote || res.NewFolder != "" {
		t.Errorf("handed off to %s (new folder %q), want the existing clone %s", res.ToDir, res.NewFolder, f.remote)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("server status:\n%s\nwant:\n%s", got, want)
	}
	if got := f.read(f.remote, ".env"); got != "SECRET=1" {
		t.Errorf(".env on the server = %q", got)
	}
	remoteSlug, _ := projectSlug(f.remote)
	if got := f.read(filepath.Join(f.home, ".claude", "projects", remoteSlug), id+".jsonl"); !strings.Contains(got, f.local) {
		t.Errorf("transcript on the server = %q", got)
	}
	if ref := f.git(f.local, "rev-parse", "-q", "--verify", parkedRef); ref == "" {
		t.Error("the laptop checkout wasn't marked as handed off")
	}
	windows, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{session_name} #{window_name}").Output()
	if !strings.Contains(string(windows), "acme api · claude") {
		t.Errorf("tmux windows = %q, want the session's own tab in the acme session", windows)
	}
}

func TestAFailedHandoffLeavesNoTabAndARetryPicksUpFromIt(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)
	f.messUp(f.local)
	id := "0f0f0f0f-1111-2222-3333-777777777777"
	slug, _ := projectSlug(f.local)
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	h := Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, Launcher: "tmux", LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}
	_, err := h.Run()
	if err == nil || !strings.Contains(err.Error(), "transcript") || !strings.Contains(err.Error(), "trying again updates it") {
		t.Fatalf("err = %v, want a transcript failure that says a retry updates the copy", err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) > 0 {
		t.Errorf("a failed handoff left tmux windows: %s", windows)
	}
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	f.write(f.local, "kept.txt", "edited while the session kept running here")
	if _, err := h.Run(); err != nil {
		t.Fatalf("the retry failed: %v", err)
	}
	if got := f.read(f.remote, "kept.txt"); got != "edited while the session kept running here" {
		t.Errorf("kept.txt on the server = %q", got)
	}
}

func TestRexSplitsBesideTheGivenPaneWithoutTakingFocus(t *testing.T) {
	f := newFixture(t)
	f.tool("rex", "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/rex-args\"\necho '{\"block_ids\":[\"block:new\"]}'\n")
	id, err := Rex{server: "server", remote: f.remoteHost()}.Split(Tab{Session: "session:s"}, "block:claude", true, "pnpm dev", "/srv/api", []string{"sh", "-c", "x"})
	if err != nil || id != "block:new" {
		t.Fatalf("split = %q, %v", id, err)
	}
	args := strings.Fields(f.read(f.home, "rex-args"))
	want := "-S server split -s session:s -b block:claude --focus=false --label pnpm dev -c /srv/api --json --split below --ratio 35 -- sh -c x"
	if got := strings.Join(args, " "); got != want {
		t.Errorf("rex %s\nwant rex %s", got, want)
	}
}

const fakeRexWindows = `#!/bin/sh
printf '%s\n' "$*" >> "$HOME/rex-calls"
case "$*" in
*"window ls"*) cat "$HOME/rex-windows" ;;
esac
`

func TestClosingTheLastTabOfARexSessionRemovesTheSession(t *testing.T) {
	f := newFixture(t)
	f.tool("rex", fakeRexWindows)
	x := Rex{server: "server", remote: f.remoteHost()}

	f.write(f.home, "rex-windows", `{"windows":[{"window_id":"window:other"}]}`)
	if err := x.Discard(Tab{Session: "session:s", Window: "window:task"}); err != nil {
		t.Fatal(err)
	}
	f.write(f.home, "rex-windows", `{"windows":[]}`)
	if err := x.Discard(Tab{Session: "session:s", Window: "window:last"}); err != nil {
		t.Fatal(err)
	}
	want := strings.Join([]string{
		"-S server window close -s session:s window:task",
		"-S server window ls -s session:s --json",
		"-S server window close -s session:s window:last",
		"-S server window ls -s session:s --json",
		"-S server kill session:s",
	}, "\n")
	if got := strings.TrimSpace(f.read(f.home, "rex-calls")); got != want {
		t.Errorf("rex calls:\n%s\nwant:\n%s", got, want)
	}
}
