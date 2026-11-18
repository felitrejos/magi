//go:build contract

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
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

func cleanEnv() []string {
	var env []string
	for _, kv := range os.Environ() {
		if strings.HasPrefix(kv, "CLAUDE_CODE_") || strings.HasPrefix(kv, "CLAUDECODE=") {
			continue
		}
		env = append(env, kv)
	}
	return env
}

type claudeResult struct {
	SessionID string `json:"session_id"`
	Result    string `json:"result"`
	IsError   bool   `json:"is_error"`
}

func startClaude(t *testing.T, dir string, args ...string) (*exec.Cmd, *bytes.Buffer) {
	t.Helper()
	if _, err := exec.LookPath("claude"); err != nil {
		t.Skip("claude isn't installed")
	}
	cmd := exec.Command("claude", append([]string{"-p", "--model", "haiku", "--output-format", "json"}, args...)...)
	cmd.Dir, cmd.Env = dir, cleanEnv()
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	return cmd, &out
}

func finishClaude(t *testing.T, cmd *exec.Cmd, out *bytes.Buffer) claudeResult {
	t.Helper()
	if err := cmd.Wait(); err != nil {
		t.Fatalf("claude failed: %v\n%s", err, out)
	}
	var res claudeResult
	if err := json.Unmarshal(out.Bytes(), &res); err != nil || res.IsError || res.SessionID == "" {
		t.Fatalf("claude's reply isn't usable: %v\n%s", err, out)
	}
	return res
}

func runClaude(t *testing.T, dir string, args ...string) claudeResult {
	t.Helper()
	cmd, out := startClaude(t, dir, args...)
	return finishClaude(t, cmd, out)
}

func TestContractClaudeRegistersALiveSession(t *testing.T) {
	dir := contractDir(t)
	cmd, out := startClaude(t, dir, "Run the shell command sleep 8, then reply with just: done", "--allowedTools", "Bash(sleep:*)")
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	var live *liveSession
	for deadline := time.Now().Add(20 * time.Second); live == nil && time.Now().Before(deadline); time.Sleep(300 * time.Millisecond) {
		listing, err := sh(local, sessionScript, "live")
		if err != nil {
			t.Fatal(err)
		}
		for _, record := range splitNull(listing) {
			var s liveSession
			if json.Unmarshal([]byte(record), &s) == nil && s.Cwd == dir && s.Status != "" {
				live = &s
			}
		}
	}
	if live == nil {
		cmd.Wait()
		t.Fatalf("no live session in ~/.claude/sessions ran in %s\n%s", dir, out)
	}
	if live.PID <= 0 {
		t.Errorf("pid = %d", live.PID)
	}
	if !sessionIDPattern.MatchString(live.SessionID) || live.StartedAt <= 0 || live.since() == "" {
		t.Errorf("the registry entry lacks what magi reads: %+v", *live)
	}
	switch live.Status {
	case "busy", "waiting", "idle":
	default:
		t.Errorf("status = %q, magi knows busy, waiting and idle", live.Status)
	}
	if _, err := sh(local, sessionScript, "running", live.SessionID); exitCode(err) != 10 {
		t.Errorf("session.sh running didn't see the live session: %v", err)
	}
	res := finishClaude(t, cmd, out)
	if res.SessionID != live.SessionID {
		t.Errorf("registry session %s, reply session %s", live.SessionID, res.SessionID)
	}
}

func TestContractClaudeResumesAMovedTranscript(t *testing.T) {
	from, to := contractDir(t), contractDir(t)
	first := runClaude(t, from, "Remember the word KUMQUAT. Reply with just: ok")
	local, err := newLocal(context.Background())
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
		t.Fatalf("the resumed session didn't remember: %q", second.Result)
	}
}

func TestContractClaudeHooks(t *testing.T) {
	dir := contractDir(t)
	hooks := filepath.Join(dir, "hooks")
	if err := os.Mkdir(hooks, 0o700); err != nil {
		t.Fatal(err)
	}
	stop := filepath.Join(hooks, "stop.sh")
	prompt := filepath.Join(hooks, "prompt.sh")
	write := func(path, body string) {
		if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	write(stop, `cat > "`+hooks+`/stop.json"
if [ ! -e "`+hooks+`/blocked" ]; then
  touch "`+hooks+`/blocked"
  echo '{"decision":"block","reason":"Now reply with just the word STOPPED."}'
fi
`)
	write(prompt, `cat > "`+hooks+`/prompt.json"
echo '{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"End every reply with the word GRAPEFRUIT."}}'
`)
	settings, _ := json.Marshal(map[string]any{"hooks": map[string]any{
		"Stop":             []any{map[string]any{"hooks": []any{map[string]any{"type": "command", "command": stop}}}},
		"UserPromptSubmit": []any{map[string]any{"hooks": []any{map[string]any{"type": "command", "command": prompt}}}},
	}})
	res := runClaude(t, dir, "--settings", string(settings), "Reply with just: hi")
	var input stopInput
	raw, err := os.ReadFile(filepath.Join(hooks, "stop.json"))
	if err != nil {
		t.Fatalf("the Stop hook didn't run: %v", err)
	}
	if err := json.Unmarshal(raw, &input); err != nil {
		t.Fatal(err)
	}
	if input.SessionID != res.SessionID || input.Cwd != dir {
		t.Errorf("Stop hook input = %+v, want session %s in %s", input, res.SessionID, dir)
	}
	if _, err := os.Stat(input.TranscriptPath); err != nil {
		t.Errorf("the Stop hook's transcript_path isn't there: %v", err)
	}
	if _, err := os.ReadFile(filepath.Join(hooks, "prompt.json")); err != nil {
		t.Errorf("the UserPromptSubmit hook didn't run: %v", err)
	}
	if !strings.Contains(strings.ToUpper(res.Result), "STOPPED") {
		t.Errorf("blocking the Stop hook didn't make Claude go on: %q", res.Result)
	}
	transcript, _ := os.ReadFile(input.TranscriptPath)
	if !bytes.Contains(bytes.ToUpper(transcript), []byte("GRAPEFRUIT")) {
		t.Errorf("additionalContext from the UserPromptSubmit hook didn't reach Claude")
	}
}

func TestContractCodexResumesAMovedSession(t *testing.T) {
	if _, err := exec.LookPath("codex"); err != nil {
		t.Skip("codex isn't installed")
	}
	from, to := contractDir(t), contractDir(t)
	run := func(dir string, args ...string) string {
		t.Helper()
		cmd := exec.Command("codex", append([]string{"exec", "--json", "--skip-git-repo-check", "-s", "read-only"}, args...)...)
		cmd.Dir = dir
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("codex failed: %v\n%s", err, out)
		}
		return string(out)
	}
	out := run(from, "Remember the word PERSIMMON. Reply with just: ok")
	var started struct {
		ThreadID string `json:"thread_id"`
	}
	for line := range strings.SplitSeq(out, "\n") {
		if strings.Contains(line, `"thread.started"`) && json.Unmarshal([]byte(line), &started) == nil {
			break
		}
	}
	if started.ThreadID == "" {
		t.Fatalf("codex didn't report a thread id:\n%s", out)
	}
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	s, err := findCodexSession(local, started.ThreadID)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Remove(s.Transcript) })
	if s.Cwd != from {
		t.Fatalf("magi read the Codex session's folder as %q, want %q", s.Cwd, from)
	}
	answer := run(to, "-c", `tui.resume_cwd="current"`, "resume", started.ThreadID, "What word did I ask you to remember? Reply with just the word.")
	if !strings.Contains(strings.ToUpper(answer), "PERSIMMON") {
		t.Fatalf("the resumed Codex session didn't remember:\n%s", answer)
	}
}
