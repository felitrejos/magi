package main

import (
	"context"
	"crypto/rand"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/felitrejos/magi/remote/internal/sh"
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
	tm := sh.Tmux{Machine: f.remoteHost()}

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
	id := "0f0f0f0f-1111-2222-3333-444444444444"
	h := f.handoffToServer(id)
	f.messUp(f.local)
	res, err := h.Run(context.Background())
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
	h := f.handoffToServer("0f0f0f0f-1111-2222-3333-777777777777")
	f.messUp(f.local)
	aside := filepath.Join(f.home, "aside.jsonl")
	if err := os.Rename(h.Session.Transcript, aside); err != nil {
		t.Fatal(err)
	}
	_, err := h.Run(context.Background())
	if err == nil || !strings.Contains(err.Error(), "transcript") || !strings.Contains(err.Error(), "trying again updates it") {
		t.Fatalf("err = %v, want a transcript failure that says a retry updates the copy", err)
	}
	if windows, _ := exec.Command("tmux", "list-windows", "-a").Output(); len(windows) > 0 {
		t.Errorf("a failed handoff left tmux windows: %s", windows)
	}
	if err := os.Rename(aside, h.Session.Transcript); err != nil {
		t.Fatal(err)
	}
	f.write(f.local, "kept.txt", "edited while the session kept running here")
	if _, err := h.Run(context.Background()); err != nil {
		t.Fatalf("the retry failed: %v", err)
	}
	if got := f.read(f.remote, "kept.txt"); got != "edited while the session kept running here" {
		t.Errorf("kept.txt on the server = %q", got)
	}
}

func (f *fixture) copyThatWeighs(dir, transcript string) (sent int64, copying func(Result) float64) {
	f.t.Helper()
	work := make([]byte, 200_000)
	rand.Read(work)
	f.write(dir, "kept.txt", string(work))
	pad := `{"pad":"` + strings.Repeat("x", 150_000) + `"}` + "\n"
	f.write(filepath.Dir(transcript), filepath.Base(transcript), f.read(filepath.Dir(transcript), filepath.Base(transcript))+pad)
	return int64(len(work) + len(pad)), func(res Result) (seconds float64) {
		for _, s := range res.Steps {
			if s.Name == "sync" || s.Name == "transcript" {
				seconds += s.Took.Seconds()
			}
		}
		return seconds
	}
}

func (f *fixture) loggedCopy(kind string, sent int64, seconds float64) {
	f.t.Helper()
	entries, err := readLog()
	if err != nil || len(entries) == 0 {
		f.t.Fatalf("log = %+v, %v", entries, err)
	}
	if e := entries[len(entries)-1]; e.Kind != kind || e.Bytes < sent || e.Bytes > sent+100_000 || e.Seconds != seconds {
		f.t.Errorf("logged %s of %d bytes in %gs, want at least the %d bytes of work and transcript in the %gs of sync and transcript", e.Kind, e.Bytes, e.Seconds, sent, seconds)
	}
}

func TestAHandoffLogsTheBytesItCopiedAndHowLongTheCopyTook(t *testing.T) {
	f := newFixture(t)
	h := f.handoffToServer("0f0f0f0f-1111-2222-3333-444444444445")
	sent, copying := f.copyThatWeighs(f.local, h.Session.Transcript)
	res, err := h.Run(context.Background())
	if err != nil {
		t.Fatalf("handoff failed after %s: %v", res.Summary(), err)
	}
	f.loggedCopy("push", sent, copying(res))
}
