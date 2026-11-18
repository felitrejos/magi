package main

import (
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

const fakeCodexThatStarts = `#!/bin/sh
printf '%s\n' "$@" > "$HOME/codex-args"
while [ "$1" != resume ]; do shift; done
f=$(find "${CODEX_HOME:-$HOME/.codex}/sessions" -name "rollout-*-$2.jsonl")
printf '%s\n' '{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}' '{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete"}}' >> "$f"
sleep 60
`

const rolloutName = "2026/11/11/rollout-2026-11-11T00-28-20-"

func (f *fixture) remoteCodexHome() string {
	return filepath.Join(f.home, "server-codex")
}

func (f *fixture) codexHost(codex bool) {
	f.t.Helper()
	f.isolatedTmux()
	f.t.Setenv("MAGI_TEST_REMOTE_CODEX_HOME", f.remoteCodexHome())
	claude := f.tool("claude", fakeClaudeThatStarts)
	path := ""
	if codex {
		path = "codex = \"" + f.tool("codex", fakeCodexThatStarts) + "\"\n"
	}
	f.config(`local_roots = ["~/laptop"]

[hosts.server]
ssh = "fakehost"
claude = "` + claude + `"
` + path + `roots = ["~/developer"]
secrets = [".env"]
launcher = "tmux"
`)
}

const longAgo = "2026-11-11T00:28:20.000Z"

func rollout(id, dir, began string, busy bool) string {
	lines := []string{
		`{"timestamp":"t","ordinal":0,"type":"session_meta","payload":{"id":"` + id + `","cwd":"` + dir + `","runtime_workspace_roots":["` + dir + `"]}}`,
		`{"timestamp":"` + began + `","ordinal":1,"type":"event_msg","payload":{"type":"task_started"}}`,
		`{"timestamp":"t","ordinal":2,"type":"response_item","payload":{"type":"message","cwd":"` + dir + `","content":[{"type":"input_text","text":"read ` + dir + `/kept.txt"}]}}`,
		`{"timestamp":"t","ordinal":3,"type":"turn_context","payload":{"cwd":"` + dir + `","writable_roots":["` + dir + `/.git","` + dir + `-old"],"note":"ran in ` + dir + `","quoted":"say \"` + dir + `\""}}`,
	}
	if !busy {
		lines = append(lines, `{"timestamp":"t","ordinal":4,"type":"event_msg","payload":{"type":"task_complete"}}`)
	}
	return strings.Join(lines, "\n") + "\n"
}

func (f *fixture) codexRollout(codexHome, id, dir, began string, busy bool) string {
	f.t.Helper()
	path := filepath.Join(codexHome, "sessions", rolloutName+id+".jsonl")
	f.write(filepath.Dir(path), filepath.Base(path), rollout(id, dir, began, busy))
	return path
}

type tui struct {
	proc   *exec.Cmd
	exited chan struct{}
}

func (f *fixture) codexTUI(dir string, args ...string) tui {
	f.t.Helper()
	f.write(filepath.Join(f.home, "tui"), "codex", "#!/bin/sh\nsleep 60\n")
	program := filepath.Join(f.home, "tui", "codex")
	if err := os.Chmod(program, 0o755); err != nil {
		f.t.Fatal(err)
	}
	proc := exec.Command(program, args...)
	proc.Dir = dir
	if err := proc.Start(); err != nil {
		f.t.Fatal(err)
	}
	t := tui{proc: proc, exited: make(chan struct{})}
	go func() { proc.Wait(); close(t.exited) }()
	f.t.Cleanup(func() { proc.Process.Kill(); <-t.exited })
	return t
}

func (t tui) exitsWithin(d time.Duration) bool {
	select {
	case <-t.exited:
		return true
	case <-time.After(d):
		return false
	}
}

func (f *fixture) codexAppServer(codexHome, id string) {
	f.t.Helper()
	lock := filepath.Join(codexHome, "thread-writer-locks", id+".lock")
	f.write(filepath.Dir(lock), filepath.Base(lock), "")
	f.write(filepath.Join(f.home, "daemon"), "codex", "#!/bin/sh\nexec 3<\"$2\"\nsleep 60 3<&-\n")
	daemon := filepath.Join(f.home, "daemon", "codex")
	if err := os.Chmod(daemon, 0o755); err != nil {
		f.t.Fatal(err)
	}
	server := exec.Command(daemon, "app-server", lock)
	if err := server.Start(); err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { server.Process.Kill(); server.Wait() })
	time.Sleep(200 * time.Millisecond)
}

type lockedBuffer struct {
	mu sync.Mutex
	b  strings.Builder
}

func (l *lockedBuffer) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.Write(p)
}

func (l *lockedBuffer) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.String()
}

func TestRewritingARolloutMovesItsSettingsButNotTheConversation(t *testing.T) {
	got, err := rewriteRollout([]byte(rollout("x", "/laptop/api", longAgo, false)), "/laptop/api", "/srv/acme/api")
	if err != nil {
		t.Fatal(err)
	}
	want := rollout("x", "/srv/acme/api", longAgo, false)
	want = strings.Replace(want, `"cwd":"/srv/acme/api","content":[{"type":"input_text","text":"read /srv/acme/api/kept.txt"}]`, `"cwd":"/laptop/api","content":[{"type":"input_text","text":"read /laptop/api/kept.txt"}]`, 1)
	want = strings.NewReplacer(`"/srv/acme/api-old"`, `"/laptop/api-old"`, `"ran in /srv/acme/api"`, `"ran in /laptop/api"`, `say \"/srv/acme/api\"`, `say \"/laptop/api\"`).Replace(want)
	if string(got) != want {
		t.Errorf("rewritten:\n%s\nwant:\n%s", got, want)
	}
}

func TestMovingAnIdleCodexSessionReopensItThere(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	f.messUp(f.local)
	f.write(filepath.Join(f.home, ".codex"), "config.toml", "model = \"x\"\n\n[projects.\""+f.local+"\"]\ntrust_level = \"trusted\"\n")
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71d6"
	local := f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, longAgo, false)
	codex := f.codexTUI(f.local, "resume", id)

	var out strings.Builder
	if err := moveCommand([]string{id, "server", "--json"}, &out); err != nil {
		t.Fatalf("move: %v\n%s", err, out.String())
	}
	events := moveEvents(t, out.String())
	if last := events[len(events)-1]; events[0].Event != "moving" || last.Event != "moved" || !last.OK || last.ToDir != f.remote {
		t.Fatalf("events = %+v", events)
	}
	if !codex.exitsWithin(5 * time.Second) {
		t.Error("Codex wasn't stopped here")
	}
	if got := f.read(filepath.Dir(local), filepath.Base(local)+".handed-off-to-server"); got != rollout(id, f.local, longAgo, false) {
		t.Errorf("the rollout here wasn't retired unchanged: %q", got)
	}
	there, err := rewriteRollout([]byte(rollout(id, f.local, longAgo, false)), f.local, f.remote)
	if err != nil {
		t.Fatal(err)
	}
	if got := f.read(filepath.Join(f.remoteCodexHome(), "sessions"), rolloutName+id+".jsonl"); !strings.HasPrefix(got, string(there)) || !strings.HasSuffix(got, `"task_complete"}}`+"\n") {
		t.Errorf("rollout on the server = %q", got)
	}
	args := f.read(f.home, "codex-args")
	if !strings.Contains(args, "tui.resume_cwd=\"current\"\nresume\n"+id+"\n[magi] This session was just handed off") || strings.Contains(args, "project memory") {
		t.Errorf("codex ran with %q", args)
	}
	if got := f.read(f.remoteCodexHome(), "config.toml"); !strings.Contains(got, "[projects.\""+f.remote+"\"]\ntrust_level = \"trusted\"") {
		t.Errorf("Codex config on the server = %q", got)
	}
	if got, want := f.git(f.remote, "status", "--porcelain"), f.git(f.local, "status", "--porcelain"); got != want {
		t.Errorf("server status:\n%s\nwant:\n%s", got, want)
	}
	if last, err := lastHandoffTo("server"); err != nil || last != id {
		t.Errorf("log = %q, %v", last, err)
	}
}

func TestMovingABusyCodexSessionWaitsForItsTurnToEnd(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71d7"
	local := f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, time.Now().Add(time.Minute).UTC().Format(time.RFC3339Nano), true)
	codex := f.codexTUI(f.local, "resume", id)

	var out lockedBuffer
	done := make(chan error, 1)
	go func() { done <- moveCommand([]string{id, "server", "--json"}, &out) }()
	deadline := time.Now().Add(10 * time.Second)
	for !strings.Contains(out.String(), `"event":"waiting"`) {
		if time.Now().After(deadline) {
			t.Fatalf("move didn't wait for the turn: %s", out.String())
		}
		time.Sleep(50 * time.Millisecond)
	}
	time.Sleep(700 * time.Millisecond)
	if len(moveEvents(t, out.String())) != 1 {
		t.Fatalf("move went ahead mid-turn: %s", out.String())
	}
	fh, err := os.OpenFile(local, os.O_APPEND|os.O_WRONLY, 0)
	if err != nil {
		t.Fatal(err)
	}
	fh.WriteString(`{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete"}}` + "\n")
	fh.Close()
	select {
	case err := <-done:
		if events := moveEvents(t, out.String()); err != nil || events[len(events)-1].Event != "moved" {
			t.Fatalf("move: %v\n%s", err, out.String())
		}
	case <-time.After(60 * time.Second):
		t.Fatal("move didn't go ahead when the turn ended")
	}
	if !codex.exitsWithin(5 * time.Second) {
		t.Error("Codex wasn't stopped here")
	}
}

func TestACodexMoveIsRefusedWhenItCantTellWhatIsRunning(t *testing.T) {
	f := newFixture(t)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71d8"
	local := f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, longAgo, false)
	for _, c := range []struct {
		codexPath bool
		want      string
	}{
		{false, "set codex to the path of codex on server under [hosts.server]"},
		{true, "can't tell whether it's session 01a1c7ca; quit Codex there"},
	} {
		f.codexHost(c.codexPath)
		other := f.codexTUI(f.local)
		var out strings.Builder
		if err := moveCommand([]string{id, "server", "--json"}, &out); err != errReported {
			t.Errorf("err = %v", err)
		}
		if events := moveEvents(t, out.String()); !strings.Contains(events[len(events)-1].Message, c.want) {
			t.Errorf("events = %+v, want %q", events, c.want)
		}
		if other.exitsWithin(300 * time.Millisecond) {
			t.Error("the move stopped a Codex it couldn't place")
		}
		other.proc.Process.Kill()
		<-other.exited
	}
	if got := f.read(filepath.Dir(local), filepath.Base(local)); got != rollout(id, f.local, longAgo, false) {
		t.Errorf("the rollout changed: %q", got)
	}
}

func TestPullBringsACodexSessionBackFromTheServer(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	f.messUp(f.local)
	ws := f.workspace(f.local)
	if err := f.push(f.remoteHost(), ws, Placement{Root: f.remote}); err != nil {
		t.Fatal(err)
	}
	if err := park(f.machine(), ws.Root, ws); err != nil {
		t.Fatal(err)
	}
	f.write(f.remote, "edited.txt", "finished on the server")
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71d9"
	remote := f.codexRollout(f.remoteCodexHome(), id, f.remote, longAgo, false)

	res, err := Pull{HostName: "server", Host: Host{SSH: "fakehost", Roots: []string{"~/developer"}, LocalRoots: []string{"~/laptop"}, Secrets: []string{".env"}}, SessionID: id, Out: io.Discard}.Run()
	if err != nil {
		t.Fatalf("pull failed after %s: %v", res.Summary(), err)
	}
	prompt := handoffPrompt(res.PromptInfo)
	if !res.Codex || res.ToDir != f.local || !strings.Contains(prompt, "handed off from server") || strings.Contains(prompt, "project memory") {
		t.Errorf("pulled = %+v", res)
	}
	back, err := rewriteRollout([]byte(rollout(id, f.remote, longAgo, false)), f.remote, f.local)
	if err != nil {
		t.Fatal(err)
	}
	if got := f.read(filepath.Join(f.home, ".codex", "sessions"), rolloutName+id+".jsonl"); got != string(back) {
		t.Errorf("rollout here = %q", got)
	}
	if got := f.read(filepath.Dir(remote), filepath.Base(remote)+".handed-off-to-"+f.machine().Name()); got == "<missing>" {
		t.Error("the server's rollout wasn't retired")
	}
	if _, err := findSession(f.remoteHost(), id); err == nil || !strings.Contains(err.Error(), "no session") {
		t.Errorf("the retired rollout still counts as the session there: %v", err)
	}
	if got := f.read(f.local, "edited.txt"); got != "finished on the server" {
		t.Errorf("edited.txt here = %q", got)
	}
}

func TestACodexHandoffOpensItsOwnCodexTab(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71da"
	local := f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, longAgo, false)
	host, err := loadHost("server")
	if err != nil {
		t.Fatal(err)
	}
	res, err := Handoff{HostName: "server", Host: host, SessionID: id, TranscriptPath: local, Cwd: f.local, Codex: true, Rollout: rolloutName + id + ".jsonl"}.Run(context.Background())
	if err != nil {
		t.Fatalf("handoff failed after %s: %v", res.Summary(), err)
	}
	windows, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{session_name} #{window_name}").Output()
	if !strings.Contains(string(windows), "acme api · codex") {
		t.Errorf("tmux windows = %q", windows)
	}
	if err := stopCodexSession(f.remoteHost(), id); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		windows, _ := exec.Command("tmux", "list-windows", "-a").Output()
		if len(windows) == 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("stopping the session there left %s", windows)
		}
		time.Sleep(100 * time.Millisecond)
	}
}

func TestATurnLeftOpenByAnEarlierCodexDoesntHoldUpTheMove(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71db"
	f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, longAgo, true)
	codex := f.codexTUI(f.local, "resume", id)
	var out strings.Builder
	if err := moveCommand([]string{id, "server", "--json"}, &out); err != nil {
		t.Fatalf("move: %v\n%s", err, out.String())
	}
	if events := moveEvents(t, out.String()); events[0].Event != "moving" || events[len(events)-1].Event != "moved" {
		t.Errorf("events = %+v", events)
	}
	if !codex.exitsWithin(5 * time.Second) {
		t.Error("Codex wasn't stopped here")
	}
}

func TestACodexMoveIsRefusedWhileTheOtherMachineStillHasItOpen(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71dc"
	local := f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, longAgo, false)
	codex := f.codexTUI(f.local, "resume", id)
	f.codexAppServer(f.remoteCodexHome(), id)

	var out strings.Builder
	if err := moveCommand([]string{id, "server", "--json"}, &out); err != errReported {
		t.Fatalf("move: %v\n%s", err, out.String())
	}
	if events := moveEvents(t, out.String()); !strings.Contains(events[len(events)-1].Message, "background server on fakehost") {
		t.Errorf("events = %+v", events)
	}
	if codex.exitsWithin(300 * time.Millisecond) {
		t.Error("the refused move stopped Codex here")
	}
	if got := f.read(filepath.Dir(local), filepath.Base(local)); got != rollout(id, f.local, longAgo, false) {
		t.Errorf("the rollout here changed: %q", got)
	}
}

func TestACodexMoveIsRefusedWhileTheAppHereHasItOpen(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71dd"
	local := f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, longAgo, false)
	f.codexAppServer(filepath.Join(f.home, ".codex"), id)

	var out strings.Builder
	if err := moveCommand([]string{id, "server", "--json"}, &out); err != errReported {
		t.Fatalf("move: %v\n%s", err, out.String())
	}
	if events := moveEvents(t, out.String()); !strings.Contains(events[len(events)-1].Message, "background server on "+f.machine().Name()) {
		t.Errorf("events = %+v", events)
	}
	if got := f.read(filepath.Dir(local), filepath.Base(local)); got != rollout(id, f.local, longAgo, false) {
		t.Errorf("the rollout here changed: %q", got)
	}
	if got := f.read(filepath.Join(f.remoteCodexHome(), "sessions"), rolloutName+id+".jsonl"); got != "<missing>" {
		t.Errorf("the rollout reached the server: %q", got)
	}
}

func TestPullIsRefusedWhileTheAppThereHasTheCodexSessionOpen(t *testing.T) {
	f := newFixture(t)
	f.codexHost(true)
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71de"
	remote := f.codexRollout(f.remoteCodexHome(), id, f.remote, longAgo, false)
	f.codexAppServer(f.remoteCodexHome(), id)

	_, err := Pull{HostName: "server", Host: Host{SSH: "fakehost", Roots: []string{"~/developer"}, LocalRoots: []string{"~/laptop"}, Secrets: []string{".env"}}, SessionID: id, Out: io.Discard}.Run()
	if err == nil || !strings.Contains(err.Error(), "background server on fakehost") {
		t.Fatalf("pull = %v", err)
	}
	if got := f.read(filepath.Dir(remote), filepath.Base(remote)); got != rollout(id, f.remote, longAgo, false) {
		t.Errorf("the rollout there changed: %q", got)
	}
}

func TestABusyCodexTurnIsntTakenForStaleWhenPsSpeaksAnotherLanguage(t *testing.T) {
	f := newFixture(t)
	t.Setenv("LC_ALL", "fr_FR.UTF-8")
	id := "01a1c7ca-8144-7e23-82c1-87dc991d71df"
	f.codexRollout(filepath.Join(f.home, ".codex"), id, f.local, time.Now().Add(time.Minute).UTC().Format(time.RFC3339Nano), true)
	f.codexTUI(f.local, "resume", id)
	s, err := findCodexSession(f.machine(), id)
	if err != nil {
		t.Fatal(err)
	}
	if len(s.Running) != 1 || s.StaleTurn != "" {
		t.Errorf("session = %+v", s)
	}
}
