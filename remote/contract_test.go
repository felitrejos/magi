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
	agents := exec.Command("claude", "agents", "--json")
	agents.Env = cleanEnv()
	listing, err := agents.Output()
	if err != nil {
		t.Fatalf("claude agents --json failed: %v", err)
	}
	var listed []liveSession
	if err := json.Unmarshal(listing, &listed); err != nil {
		t.Fatalf("claude agents --json isn't a list of sessions: %v\n%s", err, listing)
	}
	i := slices.IndexFunc(listed, func(s liveSession) bool { return s.SessionID == live.SessionID })
	if i < 0 {
		t.Fatalf("claude agents --json doesn't list the live session %s:\n%s", live.SessionID, listing)
	}
	if got := listed[i]; got.PID != live.PID || got.Cwd != dir || got.StartedAt != live.StartedAt {
		t.Errorf("claude agents --json lists %+v, the registry has %+v", got, *live)
	}
	switch listed[i].Status {
	case "busy", "waiting", "idle":
	default:
		t.Errorf("claude agents --json status = %q, magi knows busy, waiting and idle", listed[i].Status)
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

func TestContractClaudeStopHookSeesItsProcessAndBackgroundTasks(t *testing.T) {
	dir := contractDir(t)
	hooks := filepath.Join(dir, "hooks")
	if err := os.Mkdir(hooks, 0o700); err != nil {
		t.Fatal(err)
	}
	stop := filepath.Join(hooks, "stop.sh")
	body := `#!/bin/sh
cat >> "` + hooks + `/stop.jsonl"
echo "pid=$CLAUDE_PID" >> "` + hooks + `/pids"
if [ ! -e "` + hooks + `/fed" ]; then
  touch "` + hooks + `/fed"
  echo '{"hookSpecificOutput":{"hookEventName":"Stop","additionalContext":"Now reply with just the word CONTINUED."}}'
fi
`
	if err := os.WriteFile(stop, []byte(body), 0o700); err != nil {
		t.Fatal(err)
	}
	settings, _ := json.Marshal(map[string]any{"hooks": map[string]any{
		"Stop": []any{map[string]any{"hooks": []any{map[string]any{"type": "command", "command": stop}}}},
	}})
	cmd, out := startClaude(t, dir, "--settings", string(settings), "--allowedTools", "Bash(sleep:*)", "--",
		"Start the shell command 'sleep 20' as a background task (run_in_background true), then without waiting reply with just: started")
	finishClaude(t, cmd, out)

	pids, err := os.ReadFile(filepath.Join(hooks, "pids"))
	if err != nil {
		t.Fatalf("the Stop hook didn't run: %v", err)
	}
	for _, pid := range strings.Fields(string(pids)) {
		if pid != "pid="+strconv.Itoa(cmd.Process.Pid) {
			t.Errorf("CLAUDE_PID in the Stop hook: %s, claude ran as %d", pid, cmd.Process.Pid)
		}
	}
	raw, err := os.ReadFile(filepath.Join(hooks, "stop.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	type stopSeen struct {
		stopInput
		StopHookActive       bool   `json:"stop_hook_active"`
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
		t.Fatalf("the Stop hook ran %d times, want the turn and the one its feedback continued:\n%s", len(stops), raw)
	}
	if !slices.ContainsFunc(stops[0].BackgroundTasks, func(b backgroundTask) bool { return b.Type == "shell" && strings.Contains(b.Command, "sleep 20") }) {
		t.Errorf("the first Stop hook's background_tasks don't list the running sleep: %+v", stops[0].BackgroundTasks)
	}
	if !stops[1].StopHookActive || !strings.Contains(strings.ToUpper(stops[1].LastAssistantMessage), "CONTINUED") {
		t.Errorf("additionalContext from the Stop hook didn't make Claude go on in the same turn: %+v", stops[1])
	}
}

func TestContractClaudeSessionStartTellsAResumedSession(t *testing.T) {
	dir := contractDir(t)
	first := runClaude(t, dir, "Reply with just: ok")
	hooks := filepath.Join(dir, "hooks")
	if err := os.Mkdir(hooks, 0o700); err != nil {
		t.Fatal(err)
	}
	notes := t.TempDir()
	path := returnNotePath(notes, first.SessionID)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("[magi] The secret word is TANGERINE."), 0o600); err != nil {
		t.Fatal(err)
	}
	var reply bytes.Buffer
	if err := passReturnNote(notes, first.SessionID, "SessionStart", &reply); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(hooks, "start.out"), reply.Bytes(), 0o600); err != nil {
		t.Fatal(err)
	}
	start := filepath.Join(hooks, "start.sh")
	if err := os.WriteFile(start, []byte(`#!/bin/sh
cat > "`+hooks+`/start.json"
cat "`+hooks+`/start.out"
`), 0o700); err != nil {
		t.Fatal(err)
	}
	plugin, err := os.ReadFile(filepath.Join("..", "plugin", "hooks", "hooks.json"))
	if err != nil {
		t.Fatal(err)
	}
	var registered struct {
		Hooks map[string][]struct {
			Matcher string           `json:"matcher"`
			Hooks   []map[string]any `json:"hooks"`
		} `json:"hooks"`
	}
	if err := json.Unmarshal(plugin, &registered); err != nil {
		t.Fatal(err)
	}
	sessionStart := registered.Hooks["SessionStart"]
	for _, group := range sessionStart {
		for _, hook := range group.Hooks {
			hook["command"] = start
		}
	}
	settings, _ := json.Marshal(map[string]any{"hooks": map[string]any{"SessionStart": sessionStart}})
	second := runClaude(t, dir, "--settings", string(settings), "--resume", first.SessionID, "What is the secret word? Reply with just the word.")
	raw, err := os.ReadFile(filepath.Join(hooks, "start.json"))
	if err != nil {
		t.Fatalf("the plugin's SessionStart hook didn't run on resume: %v", err)
	}
	var input struct {
		SessionID string `json:"session_id"`
		Source    string `json:"source"`
	}
	if err := json.Unmarshal(raw, &input); err != nil {
		t.Fatal(err)
	}
	if input.SessionID != first.SessionID || input.Source != "resume" {
		t.Errorf("SessionStart input = %+v, want session %s resumed", input, first.SessionID)
	}
	if !strings.Contains(strings.ToUpper(second.Result), "TANGERINE") {
		t.Errorf("the note from the SessionStart hook didn't reach Claude: %q", second.Result)
	}
}
