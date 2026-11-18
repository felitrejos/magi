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
	"strings"
	"testing"
	"time"
)

func startCodex(t *testing.T, dir string, args ...string) (*exec.Cmd, *bytes.Buffer) {
	t.Helper()
	if _, err := exec.LookPath("codex"); err != nil {
		t.Skip("codex isn't installed")
	}
	cmd := exec.Command("codex", append([]string{"exec", "--json", "--skip-git-repo-check", "-s", "read-only"}, args...)...)
	cmd.Dir = dir
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	return cmd, &out
}

func finishCodex(t *testing.T, cmd *exec.Cmd, out *bytes.Buffer) string {
	t.Helper()
	if err := cmd.Wait(); err != nil {
		t.Fatalf("codex failed: %v\n%s", err, out)
	}
	return out.String()
}

func runCodex(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd, out := startCodex(t, dir, args...)
	return finishCodex(t, cmd, out)
}

func codexThreadID(t *testing.T, out string) string {
	t.Helper()
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
	t.Cleanup(func() {
		if out, err := exec.Command("codex", "delete", "--force", started.ThreadID).CombinedOutput(); err != nil {
			t.Errorf("deleting Codex session %s: %v\n%s", started.ThreadID, err, out)
		}
	})
	return started.ThreadID
}

func TestContractCodexResumesAMovedSession(t *testing.T) {
	from, to := contractDir(t), contractDir(t)
	id := codexThreadID(t, runCodex(t, from, "Remember the word PERSIMMON. Reply with just: ok"))
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	s, err := findCodexSession(local, id)
	if err != nil {
		t.Fatal(err)
	}
	if s.Cwd != from {
		t.Fatalf("magi read the Codex session's folder as %q, want %q", s.Cwd, from)
	}
	answer := runCodex(t, to, "-c", `tui.resume_cwd="current"`, "resume", id, "What word did I ask you to remember? Reply with just the word.")
	if !strings.Contains(strings.ToUpper(answer), "PERSIMMON") {
		t.Fatalf("the resumed Codex session didn't remember:\n%s", answer)
	}
}

func TestContractCodexResumesASessionMovedToAnotherCodexHome(t *testing.T) {
	from, to := contractDir(t), contractDir(t)
	id := codexThreadID(t, runCodex(t, from, "Remember the word QUINCE. Reply with just: ok"))
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	s, err := findCodexSession(local, id)
	if err != nil {
		t.Fatal(err)
	}
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatal(err)
	}
	auth, err := os.ReadFile(filepath.Join(home, ".codex", "auth.json"))
	if err != nil {
		t.Fatal(err)
	}
	there := t.TempDir()
	if err := os.WriteFile(filepath.Join(there, "auth.json"), auth, 0o400); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CODEX_HOME", there)
	rollout, _, err := moveRollout(local, s.Transcript, s.Rollout, from, local, to)
	if err != nil {
		t.Fatal(err)
	}
	if want := filepath.Join(there, "sessions", s.Rollout); rollout != want {
		t.Fatalf("magi wrote the rollout to %s, want %s", rollout, want)
	}
	answers, err := askCodex(local, "codex", codexCall{Method: "thread/list", Params: map[string]any{"cwd": to, "sourceKinds": []string{"exec"}}})
	if err != nil {
		t.Fatal(err)
	}
	var listed struct {
		Data []codexThread `json:"data"`
	}
	if err := json.Unmarshal(answers[0], &listed); err != nil {
		t.Fatal(err)
	}
	if !slices.ContainsFunc(listed.Data, func(c codexThread) bool { return c.ID == id }) {
		t.Errorf("Codex there doesn't list the moved session under %s: %s", to, answers[0])
	}
	answer := runCodex(t, to, "-c", `tui.resume_cwd="current"`, "resume", id, "What word did I ask you to remember? Reply with just the word.")
	if !strings.Contains(strings.ToUpper(answer), "QUINCE") {
		t.Fatalf("the Codex session moved to another CODEX_HOME didn't remember:\n%s", answer)
	}
}

func TestContractCodexListsAnOpenSession(t *testing.T) {
	dir := contractDir(t)
	cmd, out := startCodex(t, dir, "Run the shell command: sleep 8. Then reply with just: done")
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	var open *codexThread
	for deadline := time.Now().Add(30 * time.Second); open == nil && time.Now().Before(deadline); time.Sleep(500 * time.Millisecond) {
		threads, err := openCodexSessions(local, "codex")
		if err != nil {
			cmd.Wait()
			t.Fatal(err)
		}
		if i := slices.IndexFunc(threads, func(c codexThread) bool { return c.Cwd == dir }); i >= 0 {
			open = &threads[i]
		}
	}
	id := codexThreadID(t, finishCodex(t, cmd, out))
	if open == nil {
		t.Fatalf("magi didn't list the open Codex session in %s\n%s", dir, out)
	}
	if open.ID != id {
		t.Errorf("listed session %s, codex ran %s", open.ID, id)
	}
}
