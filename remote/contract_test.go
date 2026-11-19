//go:build contract

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

func contractDir(t *testing.T) string {
	t.Helper()
	dir, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatal(err)
	}
	slug, err := projectSlug(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(filepath.Join(home, ".claude", "projects", slug)) })
	return dir
}

func needClaude(t *testing.T) {
	t.Helper()
	if _, err := exec.LookPath("claude"); err != nil {
		t.Skip("claude isn't installed")
	}
}

func startClaude(t *testing.T, dir string, args ...string) (*exec.Cmd, *bytes.Buffer) {
	t.Helper()
	needClaude(t)
	cmd := exec.Command("claude", append([]string{"-p", "--model", "haiku", "--output-format", "json"}, args...)...)
	cmd.Dir, cmd.Env = dir, claudeEnv()
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	return cmd, &out
}

func finishClaude(t *testing.T, cmd *exec.Cmd, out *bytes.Buffer) claudeReply {
	t.Helper()
	if err := cmd.Wait(); err != nil {
		t.Fatalf("claude failed: %v\n%s", err, out)
	}
	var res claudeReply
	if err := json.Unmarshal(out.Bytes(), &res); err != nil || res.IsError || res.SessionID == "" {
		t.Fatalf("claude's reply isn't usable: %v\n%s", err, out)
	}
	return res
}

func runClaude(t *testing.T, dir string, args ...string) claudeReply {
	t.Helper()
	needClaude(t)
	reply, err := tryClaude(dir, args...)
	if err != nil {
		t.Fatalf("claude failed: %v", err)
	}
	return reply
}

func TestContractClaudeRegistersALiveSession(t *testing.T) {
	dir := contractDir(t)
	cmd, out := startClaude(t, dir, "Run the shell command sleep 8, then reply with just: done", "--allowedTools", "Bash(sleep:*)")
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	var live *liveSession
	for deadline := time.Now().Add(20 * time.Second); live == nil && time.Now().Before(deadline); time.Sleep(300 * time.Millisecond) {
		sessions, err := liveSessions(local)
		if err != nil {
			t.Fatal(err)
		}
		if i := slices.IndexFunc(sessions, func(s liveSession) bool { return s.Cwd == dir && s.Status == "busy" }); i >= 0 {
			live = &sessions[i]
		}
	}
	if live == nil {
		cmd.Wait()
		t.Fatalf("claude agents --json never listed a busy session in %s\n%s", dir, out)
	}
	if live.PID <= 0 || !store.SessionIDPattern.MatchString(live.SessionID) || live.StartedAt <= 0 {
		t.Errorf("claude agents --json lacks what magi reads: %+v", *live)
	}
	if live.since() == "" {
		t.Errorf("the registry has no procStart for pid %d: %+v", live.PID, *live)
	}
	if running, err := runningThere(local, live.SessionID); err != nil || !running {
		t.Errorf("runningThere = %v, %v for the live session", running, err)
	}
	if _, err := sh.Run(local, sessionScript, "stop", strconv.Itoa(live.PID), live.SessionID, live.since(), local.Claude()); sh.ExitCode(err) != sh.ExitBusy {
		t.Errorf("stopping the busy session gave %v, want it refused as busy", err)
	}
	probe, err := sh.Run(local, platformScript, local.Claude())
	if err != nil {
		t.Fatal(err)
	}
	if m := parsePlatform("here", string(probe)); m.Working < 1 {
		t.Errorf("the probe counted %d working sessions while one was busy:\n%s", m.Working, probe)
	}
	res := finishClaude(t, cmd, out)
	if res.SessionID != live.SessionID {
		t.Errorf("registry session %s, reply session %s", live.SessionID, res.SessionID)
	}
}

func hookSettings(t *testing.T, dir string, scripts map[string]string) (string, string) {
	t.Helper()
	hooks := filepath.Join(dir, "hooks")
	if err := os.Mkdir(hooks, 0o700); err != nil {
		t.Fatal(err)
	}
	registered := map[string]any{}
	for event, body := range scripts {
		path := filepath.Join(hooks, event+".sh")
		if err := os.WriteFile(path, []byte("#!/bin/sh\n"+strings.ReplaceAll(body, "HOOKS", hooks)), 0o700); err != nil {
			t.Fatal(err)
		}
		registered[event] = []any{map[string]any{"hooks": []any{map[string]any{"type": "command", "command": path}}}}
	}
	settings, _ := json.Marshal(map[string]any{"hooks": registered})
	return hooks, string(settings)
}

func TestContractClaudeResumesAMovedTranscript(t *testing.T) {
	from, to := contractDir(t), contractDir(t)
	first := runClaude(t, from, "Remember the word KUMQUAT. Reply with just: ok")
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	s, err := findSession(local, first.SessionID)
	if err != nil {
		t.Fatal(err)
	}
	if s.Cwd != from {
		t.Fatalf("magi read the session's folder as %q, want %q", s.Cwd, from)
	}
	home, _ := os.UserHomeDir()
	fromSlug, _ := projectSlug(from)
	if want := filepath.Join(home, ".claude", "projects", fromSlug, first.SessionID+".jsonl"); s.Transcript != want {
		t.Fatalf("transcript at %s, want %s", s.Transcript, want)
	}
	toSlug, _ := projectSlug(to)
	moved := filepath.Join(home, ".claude", "projects", toSlug, first.SessionID+".jsonl")
	if err := os.MkdirAll(filepath.Dir(moved), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(s.Transcript, moved); err != nil {
		t.Fatal(err)
	}
	second := runClaude(t, to, "--resume", first.SessionID, "What word did I ask you to remember? Reply with just the word.")
	if !strings.Contains(strings.ToUpper(second.Result), "KUMQUAT") {
		t.Errorf("the resumed session didn't remember: %q", second.Result)
	}
}

func TestContractClaudeHooks(t *testing.T) {
	dir := contractDir(t)
	hooks, settings := hookSettings(t, dir, map[string]string{
		"Stop": `cat >> HOOKS/stop.jsonl; echo >> HOOKS/stop.jsonl
echo "$CLAUDE_PID" >> HOOKS/pids
if [ "$(grep -c . HOOKS/pids)" = 1 ]; then echo '{"decision":"block","reason":"Now reply with just the word STOPPED."}'; fi
`,
		"UserPromptSubmit": "cat > HOOKS/prompt.json\n",
	})
	cmd, out := startClaude(t, dir, "--settings", settings, "--allowedTools", "Bash(sleep:*)", "--",
		"Start the shell command 'sleep 20' as a background task (run_in_background true), then without waiting reply with just: started")
	res := finishClaude(t, cmd, out)
	raw, err := os.ReadFile(filepath.Join(hooks, "stop.jsonl"))
	if err != nil {
		t.Fatalf("the Stop hook didn't run: %v", err)
	}
	type stopSeen struct {
		stopInput
		LastAssistantMessage string `json:"last_assistant_message"`
	}
	var stops []stopSeen
	for decoder := json.NewDecoder(bytes.NewReader(raw)); decoder.More(); {
		var s stopSeen
		if err := decoder.Decode(&s); err != nil {
			t.Fatal(err)
		}
		stops = append(stops, s)
	}
	if len(stops) < 2 {
		t.Fatalf("the Stop hook ran %d times, want the turn and the one its block continued:\n%s", len(stops), raw)
	}
	if stops[0].SessionID != res.SessionID || stops[0].Cwd != dir {
		t.Errorf("Stop hook input = %+v, want session %s in %s", stops[0].stopInput, res.SessionID, dir)
	}
	if _, err := os.Stat(stops[0].TranscriptPath); err != nil {
		t.Errorf("the Stop hook's transcript_path isn't there: %v", err)
	}
	if !slices.ContainsFunc(stops[0].BackgroundTasks, func(b backgroundTask) bool { return b.Type == "shell" && strings.Contains(b.Command, "sleep 20") }) {
		t.Errorf("background_tasks don't list the running sleep: %+v", stops[0].BackgroundTasks)
	}
	if !strings.Contains(strings.ToUpper(stops[1].LastAssistantMessage), "STOPPED") {
		t.Errorf("blocking the Stop hook didn't make Claude go on: %+v", stops[1])
	}
	pids, _ := os.ReadFile(filepath.Join(hooks, "pids"))
	for _, pid := range strings.Fields(string(pids)) {
		if pid != strconv.Itoa(cmd.Process.Pid) {
			t.Errorf("CLAUDE_PID in the Stop hook = %s, claude ran as %d", pid, cmd.Process.Pid)
		}
	}
	if _, err := os.Stat(filepath.Join(hooks, "prompt.json")); err != nil {
		t.Errorf("the UserPromptSubmit hook didn't run: %v", err)
	}
}
