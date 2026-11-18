package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestStartHookPassesTheReturnNoteOnce(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	id := "11111111-2222-3333-4444-666666666666"
	path := returnNotePath(home, id)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("[magi] back on the Mac"), 0o600); err != nil {
		t.Fatal(err)
	}
	input := `{"session_id":"` + id + `","source":"resume"}`
	var out strings.Builder
	if err := startHook(strings.NewReader(input), &out); err != nil {
		t.Fatal(err)
	}
	var got struct {
		HookSpecificOutput struct {
			HookEventName     string `json:"hookEventName"`
			AdditionalContext string `json:"additionalContext"`
		} `json:"hookSpecificOutput"`
	}
	if err := json.Unmarshal([]byte(out.String()), &got); err != nil {
		t.Fatal(err)
	}
	if got.HookSpecificOutput.HookEventName != "SessionStart" || got.HookSpecificOutput.AdditionalContext != "[magi] back on the Mac" {
		t.Fatalf("hook output = %s", out.String())
	}
	out.Reset()
	if err := promptHook(strings.NewReader(input), &out); err != nil {
		t.Fatal(err)
	}
	if out.Len() != 0 {
		t.Fatalf("the prompt hook passed the note again after the session start: %s", out.String())
	}
}

func TestTheHandoffNoteListsWhatWasStillRunning(t *testing.T) {
	var input stopInput
	stop := `{"session_id":"s","background_tasks":[
		{"id":"b1","type":"shell","status":"running","description":"Start the dev server","command":"pnpm dev"},
		{"id":"a1","type":"subagent","status":"running","description":"Migrate the billing tests","agent_type":"general-purpose"}]}`
	if err := json.Unmarshal([]byte(stop), &input); err != nil {
		t.Fatal(err)
	}
	note := stillRunning("the subagent had done a.ts and b.ts", input.BackgroundTasks)
	p := handoffPrompt(Prompt{From: "laptop", To: "server", OldDir: "/a", NewDir: "/b", Note: note})
	want := "the subagent had done a.ts and b.ts; still running when the turn ended: Start the dev server (shell pnpm dev); Migrate the billing tests (subagent)"
	if !strings.Contains(p, want) {
		t.Errorf("prompt is missing %q:\n%s", want, p)
	}
	if got := stillRunning("", input.BackgroundTasks[:1]); got != "still running when the turn ended: Start the dev server (shell pnpm dev)" {
		t.Errorf("with no note of Claude's the note is %q", got)
	}
	if got := stillRunning("", nil); got != "" {
		t.Errorf("with nothing running the note is %q", got)
	}
}
