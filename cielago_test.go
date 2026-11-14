package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

const fakeSSH = `#!/bin/sh
host=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    --) shift; break ;;
    *) if [ -z "$host" ]; then host=$1; shift; else break; fi ;;
  esac
done
exec sh -c "$*"
`

type fixture struct {
	t      *testing.T
	home   string
	local  string
	remote string
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(home, "bin")
	f := &fixture{t: t, home: home, local: filepath.Join(home, "laptop", "app"), remote: filepath.Join(home, "developer", "group", "app")}
	f.write(bin, "ssh", fakeSSH)
	if err := os.Chmod(filepath.Join(bin, "ssh"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("PATH", bin+":"+os.Getenv("PATH"))
	t.Setenv("GIT_CONFIG_GLOBAL", "/dev/null")
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_AUTHOR_NAME", "t")
	t.Setenv("GIT_AUTHOR_EMAIL", "t@t")
	t.Setenv("GIT_COMMITTER_NAME", "t")
	t.Setenv("GIT_COMMITTER_EMAIL", "t@t")

	f.git(home, "init", "-q", "-b", "main", f.local)
	f.git(f.local, "remote", "add", "origin", "git@github.com:me/app.git")
	f.write(f.local, ".gitignore", ".env*\nnode_modules/\n")
	f.write(f.local, "kept.txt", "v1")
	f.write(f.local, "edited.txt", "v1")
	f.write(f.local, "gone.txt", "v1")
	f.git(f.local, "add", ".")
	f.git(f.local, "commit", "-q", "-m", "init")
	f.git(home, "clone", "-q", f.local, f.remote)
	f.git(f.remote, "remote", "set-url", "origin", "https://github.com/me/app")
	return f
}

func (f *fixture) git(dir string, args ...string) string {
	f.t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		f.t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func (f *fixture) write(dir, name, body string) {
	f.t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) read(dir, name string) string {
	f.t.Helper()
	data, err := os.ReadFile(filepath.Join(dir, name))
	if err != nil {
		return "<missing>"
	}
	return string(data)
}

func (f *fixture) remoteHost() *Remote {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	r, err := newRemote(ctx, "fakehost")
	if err != nil {
		f.t.Fatal(err)
	}
	return r
}

func (f *fixture) workspace() Workspace {
	f.t.Helper()
	ws, err := readWorkspace(context.Background(), f.local, []string{".env", ".env.*"})
	if err != nil {
		f.t.Fatal(err)
	}
	return ws
}

func TestReadWorkspaceListsChangesDeletionsAndSecrets(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "edited.txt", "v2")
	f.write(f.local, "staged-new.txt", "new")
	f.git(f.local, "add", "staged-new.txt")
	f.write(f.local, "untracked.txt", "u")
	if err := os.Remove(filepath.Join(f.local, "gone.txt")); err != nil {
		t.Fatal(err)
	}
	f.write(f.local, ".env", "SECRET=1")
	f.write(f.local, "web/.env.local", "SECRET=2")
	f.write(f.local, "node_modules/pkg/.env", "not ours")
	f.git(f.local, "init", "-q", "vendor/nested")

	ws := f.workspace()
	slices.Sort(ws.Changed)
	slices.Sort(ws.Secrets)
	if want := []string{"edited.txt", "staged-new.txt", "untracked.txt"}; !slices.Equal(ws.Changed, want) {
		t.Errorf("Changed = %v, want %v", ws.Changed, want)
	}
	if want := []string{"gone.txt"}; !slices.Equal(ws.Deleted, want) {
		t.Errorf("Deleted = %v, want %v", ws.Deleted, want)
	}
	if want := []string{".env", "web/.env.local"}; !slices.Equal(ws.Secrets, want) {
		t.Errorf("Secrets = %v, want %v", ws.Secrets, want)
	}
	if ws.Branch != "main" || ws.Subdir != "." {
		t.Errorf("Branch = %q, Subdir = %q", ws.Branch, ws.Subdir)
	}
}

func TestHandoffSyncsCommitsChangesAndSecretsToTheHost(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "committed.txt", "from a laptop commit")
	f.git(f.local, "add", "committed.txt")
	f.git(f.local, "commit", "-q", "-m", "laptop work")
	f.write(f.local, "edited.txt", "v2")
	f.write(f.local, "dir with space/new file.txt", "new")
	if err := os.Remove(filepath.Join(f.local, "gone.txt")); err != nil {
		t.Fatal(err)
	}
	f.write(f.local, ".env", "SECRET=1")

	r := f.remoteHost()
	ws := f.workspace()
	home, clones, err := listClones(r, []string{"~/developer"})
	if err != nil {
		t.Fatal(err)
	}
	repo, err := matchClone(clones, ws, r.host)
	if err != nil {
		t.Fatal(err)
	}
	if home != f.home || repo != f.remote {
		t.Fatalf("found %q, %q; want %q, %q", home, repo, f.home, f.remote)
	}
	if err := syncWorkspace(r, ws, repo); err != nil {
		t.Fatal(err)
	}

	if got, want := f.git(f.remote, "rev-parse", "main"), f.git(f.local, "rev-parse", "HEAD"); got != want {
		t.Errorf("host main = %s, want %s", got, want)
	}
	for name, want := range map[string]string{
		"committed.txt":               "from a laptop commit",
		"edited.txt":                  "v2",
		"dir with space/new file.txt": "new",
		".env":                        "SECRET=1",
		"gone.txt":                    "<missing>",
	} {
		if got := f.read(f.remote, name); got != want {
			t.Errorf("host %s = %q, want %q", name, got, want)
		}
	}
}

func TestHandoffRefusesAHostWithUncommittedWork(t *testing.T) {
	f := newFixture(t)
	f.write(f.remote, "edited.txt", "host-only edit")

	err := syncWorkspace(f.remoteHost(), f.workspace(), f.remote)
	if err == nil || !strings.Contains(err.Error(), "uncommitted changes") {
		t.Fatalf("err = %v, want uncommitted changes refusal", err)
	}
	if got := f.read(f.remote, "edited.txt"); got != "host-only edit" {
		t.Errorf("host edit was overwritten: %q", got)
	}
}

func TestHandoffRefusesADivergedBranchWithoutRunningItsName(t *testing.T) {
	f := newFixture(t)
	branch := `x";touch${IFS}PWNED;"y`
	f.git(f.local, "check-ref-format", "--branch", branch)
	f.git(f.local, "checkout", "-q", "-b", branch)
	f.git(f.remote, "checkout", "-q", "-b", branch)
	f.write(f.remote, "host.txt", "host commit")
	f.git(f.remote, "add", "host.txt")
	f.git(f.remote, "commit", "-q", "-m", "host only")

	err := syncWorkspace(f.remoteHost(), f.workspace(), f.remote)
	if err == nil || !strings.Contains(err.Error(), "has commits this machine does not") {
		t.Fatalf("err = %v, want divergence refusal", err)
	}
	for _, dir := range []string{f.remote, f.local, f.home} {
		if _, err := os.Stat(filepath.Join(dir, "PWNED")); err == nil {
			t.Fatalf("branch name was executed as shell code (PWNED in %s)", dir)
		}
	}
}

func TestWaitForRemoteControlIgnoresBrokenAndStaleSessionFiles(t *testing.T) {
	f := newFixture(t)
	launched := time.Now()
	sessions := filepath.Join(f.home, ".claude", "sessions")
	f.write(sessions, "1.json", "{not json")
	f.write(sessions, "2.json", `{"sessionId":"s1","startedAt":1000,"bridgeSessionId":"old"}`)

	ctx, cancel := context.WithTimeout(context.Background(), 1500*time.Millisecond)
	defer cancel()
	r := f.remoteHost()
	r.ctx = ctx
	if err := waitForRemoteControl(r, "s1", launched); err == nil {
		t.Fatal("a stale session file from an earlier run counted as the new session")
	}

	f.write(sessions, "3.json", `{"sessionId":"s1","startedAt":`+strconv.FormatInt(launched.UnixMilli(), 10)+`,"bridgeSessionId":"cse_1"}`)
	if err := waitForRemoteControl(f.remoteHost(), "s1", launched); err != nil {
		t.Fatal(err)
	}
}

func TestWaitForRemoteControlWorksWithNoSessionFiles(t *testing.T) {
	f := newFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 1200*time.Millisecond)
	defer cancel()
	r := f.remoteHost()
	r.ctx = ctx
	err := waitForRemoteControl(r, "s1", time.Now())
	if err == nil || !strings.Contains(err.Error(), "in time") {
		t.Fatalf("err = %v, want a timeout rather than a script failure", err)
	}
}

func TestIsTrustedAcceptsATrustedParent(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, ".claude.json", `{"projects":{"`+f.home+`/developer":{"hasTrustDialogAccepted":true}}}`)
	r := f.remoteHost()
	if trusted, err := isTrusted(r, f.remote); err != nil || !trusted {
		t.Fatalf("trusted = %v, err = %v; want a trusted parent to count", trusted, err)
	}
	if trusted, err := isTrusted(r, filepath.Join(f.home, "elsewhere")); err != nil || trusted {
		t.Fatalf("trusted = %v, err = %v; want an untrusted folder to report false", trusted, err)
	}
}

func TestHandoffCreatesACloneForAProjectTheHostDoesNotHave(t *testing.T) {
	f := newFixture(t)
	f.write(f.local, "edited.txt", "v2")
	f.write(f.local, ".env", "SECRET=1")
	r := f.remoteHost()
	ws := f.workspace()
	_, clones, err := listClones(r, []string{"~/developer"})
	if err != nil {
		t.Fatal(err)
	}
	ws.Origin = "git@github.com:me/brand-new.git"
	if repo, err := matchClone(clones, ws, r.host); err != nil || repo != "" {
		t.Fatalf("matchClone = %q, %v; want no clone", repo, err)
	}

	dir := filepath.Join(f.home, "developer", "newgroup", "brand-new")
	if err := createClone(r, dir, ws.Origin); err != nil {
		t.Fatal(err)
	}
	if err := syncWorkspace(r, ws, dir); err != nil {
		t.Fatal(err)
	}
	if got, want := f.git(dir, "rev-parse", "HEAD"), f.git(f.local, "rev-parse", "HEAD"); got != want {
		t.Errorf("new clone HEAD = %s, want %s", got, want)
	}
	if got := f.git(dir, "branch", "--show-current"); got != "main" {
		t.Errorf("new clone branch = %q, want main", got)
	}
	if got := f.read(dir, ".env") + "|" + f.read(dir, "edited.txt"); got != "SECRET=1|v2" {
		t.Errorf("new clone files = %q", got)
	}
	if err := createClone(r, dir, ws.Origin); err == nil {
		t.Fatal("createClone overwrote a folder that already exists")
	}
}

func TestMatchCloneFindsARepoWithNoRemoteByItsFirstCommit(t *testing.T) {
	f := newFixture(t)
	f.git(f.local, "remote", "remove", "origin")
	f.git(f.remote, "remote", "remove", "origin")
	r := f.remoteHost()
	_, clones, err := listClones(r, []string{"~/developer"})
	if err != nil {
		t.Fatal(err)
	}
	repo, err := matchClone(clones, f.workspace(), r.host)
	if err != nil || repo != f.remote {
		t.Fatalf("matchClone = %q, %v; want %q", repo, err, f.remote)
	}
}

func TestLocalGroupMirrorsTheMacFolderLayout(t *testing.T) {
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	for _, dir := range []string{"Personal/stampr/stampr-ops", "Personal/kodo", "elsewhere/tool"} {
		if err := os.MkdirAll(filepath.Join(home, dir), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	roots := []string{"~/Personal"}
	for dir, want := range map[string]string{
		"Personal/stampr/stampr-ops": "stampr",
		"Personal/kodo":              "kodo",
		"elsewhere/tool":             "tool",
	} {
		got, err := localGroup(filepath.Join(home, dir), roots)
		if err != nil {
			t.Fatal(err)
		}
		if got != want {
			t.Errorf("localGroup(%s) = %q, want %q", dir, got, want)
		}
	}
}

func TestNormalizeRemoteMatchesSshAndHttpsForms(t *testing.T) {
	want := "github.com/felitrejos/stampr"
	for _, raw := range []string{
		"https://github.com/felitrejos/stampr.git",
		"https://GitHub.com/felitrejos/stampr/",
		"git@github.com:felitrejos/stampr.git",
		"ssh://git@github.com/felitrejos/stampr",
	} {
		if got := normalizeRemote(raw); got != want {
			t.Errorf("normalizeRemote(%q) = %q, want %q", raw, got, want)
		}
	}
}

func TestProjectSlugMatchesClaudeFolderNames(t *testing.T) {
	for dir, want := range map[string]string{
		"/home/felitrejos/developer/stampr/stampr-ops": "-home-felitrejos-developer-stampr-stampr-ops",
		"/home/me/café": "-home-me-caf-",
		"/home/me/a😀b":  "-home-me-a--b",
	} {
		got, err := projectSlug(dir)
		if err != nil {
			t.Fatal(err)
		}
		if got != want {
			t.Errorf("projectSlug(%q) = %q, want %q", dir, got, want)
		}
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

func waitForFile(t *testing.T, path string, within time.Duration) bool {
	t.Helper()
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			return true
		}
		time.Sleep(100 * time.Millisecond)
	}
	return false
}

func startFakeClaude(t *testing.T, seconds string) *exec.Cmd {
	t.Helper()
	cmd := exec.Command("sleep", seconds)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	go cmd.Wait()
	t.Cleanup(func() { cmd.Process.Kill() })
	return cmd
}

func TestRetireClosesATerminalSessionAndRetiresItsTranscript(t *testing.T) {
	transcript := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(transcript, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	claude := startFakeClaude(t, "60")
	if err := retire(armed{Host: "homelab", PID: claude.Process.Pid}, transcript); err != nil {
		t.Fatal(err)
	}
	if !waitForFile(t, transcript+".handed-off-to-homelab", 5*time.Second) {
		t.Fatal("transcript was not retired after the session was closed")
	}
}

func TestRetireLeavesADesktopSessionRunningUntilItIsArchived(t *testing.T) {
	transcript := filepath.Join(t.TempDir(), "s.jsonl")
	if err := os.WriteFile(transcript, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	claude := startFakeClaude(t, "3")
	if err := retire(armed{Host: "homelab", PID: claude.Process.Pid, Desktop: true}, transcript); err != nil {
		t.Fatal(err)
	}
	if waitForFile(t, transcript+".handed-off-to-homelab", 2*time.Second) {
		t.Fatal("desktop session was stopped or its transcript moved before it archived itself")
	}
	if !waitForFile(t, transcript+".handed-off-to-homelab", 4*time.Second) {
		t.Fatal("transcript was not retired after the desktop session exited")
	}
}

func TestHandedOffAsksDesktopSessionsToArchiveThemselves(t *testing.T) {
	desktop := handedOff(armed{Host: "homelab", Desktop: true}, Result{ShownOn: []string{"MacBook"}})
	if desktop["decision"] != "block" || !strings.Contains(desktop["reason"], "archive_session") {
		t.Errorf("desktop output = %v", desktop)
	}
	terminal := handedOff(armed{Host: "homelab"}, Result{ShownOn: []string{"MacBook"}})
	if _, blocks := terminal["decision"]; blocks || terminal["systemMessage"] == "" {
		t.Errorf("terminal output = %v", terminal)
	}
}

func TestStopRemoteSessionStopsTheSessionWithoutKillingItself(t *testing.T) {
	f := newFixture(t)
	sessionID := "11111111-2222-3333-4444-555555555555"
	fake := exec.Command("sh", "-c", "sleep 30; : --resume "+sessionID)
	if err := fake.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- fake.Wait() }()
	if err := stopRemoteSession(f.remoteHost(), sessionID); err != nil {
		t.Fatalf("stopRemoteSession failed: %v", err)
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		fake.Process.Kill()
		t.Fatal("the session process was not stopped")
	}
}

func TestHostRejectsRelativeRoots(t *testing.T) {
	h := Host{SSH: "s", Claude: "c", RexServer: "r", Roots: []string{"developer"}}
	if err := h.validate("homelab"); err == nil {
		t.Fatal("a relative root was accepted")
	}
	h.Roots = []string{"~/developer", "/srv/code"}
	if err := h.validate("homelab"); err != nil {
		t.Fatal(err)
	}
}

func TestReadWorkspaceNamesAWorktreeAfterItsMainCheckout(t *testing.T) {
	f := newFixture(t)
	tree := filepath.Join(f.local, ".claude", "worktrees", "feat")
	f.git(f.local, "worktree", "add", "-q", "-b", "feat", tree)
	ws, err := readWorkspace(context.Background(), tree, nil)
	if err != nil {
		t.Fatal(err)
	}
	if ws.Name != "app" || ws.MainRoot != f.local {
		t.Errorf("Name = %q, MainRoot = %q; want app, %q", ws.Name, ws.MainRoot, f.local)
	}
}

func TestServerGroupKeepsTheServersLayout(t *testing.T) {
	for clone, want := range map[string]string{
		"/home/me/developer/quorum/quorum-trader": "quorum",
		"/home/me/developer/kodo":                 "kodo",
		"/srv/other/thing":                        "other",
	} {
		if got := serverGroup(clone, []string{"~/developer"}, "/home/me"); got != want {
			t.Errorf("serverGroup(%s) = %q, want %q", clone, got, want)
		}
	}
}

func TestLocalGroupSkipsMissingRootsAndTheRootItself(t *testing.T) {
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	if err := os.MkdirAll(filepath.Join(home, "Personal", "kodo"), 0o755); err != nil {
		t.Fatal(err)
	}
	roots := []string{"~/Missing", "~/Personal"}
	if got, err := localGroup(filepath.Join(home, "Personal", "kodo"), roots); err != nil || got != "kodo" {
		t.Errorf("localGroup = %q, %v; want kodo", got, err)
	}
	if got, err := localGroup(filepath.Join(home, "Personal"), roots); err != nil || got != "Personal" {
		t.Errorf("localGroup(root itself) = %q, %v; want Personal", got, err)
	}
}
