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
	home, repo, err := findRemoteRepo(r, []string{"~/developer"}, ws.Origin)
	if err != nil {
		t.Fatal(err)
	}
	if home != f.home || repo != f.remote {
		t.Fatalf("findRemoteRepo = %q, %q; want %q, %q", home, repo, f.home, f.remote)
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

func TestCheckTrustedAcceptsATrustedParent(t *testing.T) {
	f := newFixture(t)
	f.write(f.home, ".claude.json", `{"projects":{"`+f.home+`/developer":{"hasTrustDialogAccepted":true}}}`)
	r := f.remoteHost()
	if err := checkTrusted(r, f.remote); err != nil {
		t.Fatal(err)
	}
	if err := checkTrusted(r, filepath.Join(f.home, "elsewhere")); err == nil {
		t.Fatal("an untrusted folder passed")
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
	desktop := handedOff(armed{Host: "homelab", Desktop: true}, "3.0s")
	if desktop["decision"] != "block" || !strings.Contains(desktop["reason"], "archive_session") {
		t.Errorf("desktop output = %v", desktop)
	}
	terminal := handedOff(armed{Host: "homelab"}, "3.0s")
	if _, blocks := terminal["decision"]; blocks || terminal["systemMessage"] == "" {
		t.Errorf("terminal output = %v", terminal)
	}
}
