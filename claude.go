package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf16"
)

func projectSlug(dir string) (string, error) {
	var slug strings.Builder
	for _, r := range dir {
		if r < 128 && (r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9') {
			slug.WriteRune(r)
			continue
		}
		slug.WriteString(strings.Repeat("-", utf16.RuneLen(r)))
	}
	if slug.Len() > 200 {
		return "", fmt.Errorf("%s is too long for a Claude project folder name; use a shorter path", dir)
	}
	return slug.String(), nil
}

func transcriptFiles(transcriptPath string) (dir string, files []string, err error) {
	dir = filepath.Dir(transcriptPath)
	name := filepath.Base(transcriptPath)
	files = []string{name}
	sidecar := strings.TrimSuffix(name, ".jsonl")
	_, err = os.Stat(filepath.Join(dir, sidecar))
	if os.IsNotExist(err) {
		return dir, files, nil
	}
	return dir, append(files, sidecar), err
}

type liveSession struct {
	SessionID       string  `json:"sessionId"`
	StartedAt       int64   `json:"startedAt"`
	BridgeSessionID *string `json:"bridgeSessionId"`
}

func waitForRemoteControl(r *Remote, sessionID string, launchedAt time.Time) error {
	script := `for f in "$HOME"/.claude/sessions/*.json; do [ -f "$f" ] && cat "$f" && printf '\0'; done; true`
	notBefore := launchedAt.Add(-10 * time.Second).UnixMilli()
	for {
		out, err := r.sh(script)
		if err != nil {
			return err
		}
		for _, file := range splitNull(out) {
			var s liveSession
			if json.Unmarshal([]byte(file), &s) != nil {
				continue
			}
			if s.SessionID == sessionID && s.StartedAt >= notBefore && s.BridgeSessionID != nil {
				return nil
			}
		}
		select {
		case <-r.ctx.Done():
			return fmt.Errorf("the session did not come up with Remote Control on %s in time", r.host)
		case <-time.After(500 * time.Millisecond):
		}
	}
}

func stopRemoteSession(r *Remote, sessionID string) error {
	_, err := r.sh("pkill -TERM -f -- " + quote("[-]-resume "+sessionID) + "; true")
	return err
}

func isTrusted(r *Remote, dir string) (bool, error) {
	out, err := r.sh(`cat "$HOME"/.claude.json`)
	if err != nil {
		return false, err
	}
	var state struct {
		Projects map[string]struct {
			Trusted bool `json:"hasTrustDialogAccepted"`
		} `json:"projects"`
	}
	if err := json.Unmarshal(out, &state); err != nil {
		return false, fmt.Errorf("read ~/.claude.json on %s: %w", r.host, err)
	}
	for d := dir; ; d = filepath.Dir(d) {
		if state.Projects[d].Trusted {
			return true, nil
		}
		if d == filepath.Dir(d) {
			return false, nil
		}
	}
}

type Prompt struct {
	From, To, OldDir, NewDir string
	Git                      bool
	Note                     string
	Sidecar                  string
}

func handoffPrompt(p Prompt) string {
	var b strings.Builder
	fmt.Fprintf(&b, "[cielago] This session was just handed off from %s to %s. The project is now at %s (it was %s on %s). ", p.From, p.To, p.NewDir, p.OldDir, p.From)
	if p.Git {
		b.WriteString("Branch, commits, staged and unstaged changes, secret files and project memory were synced. ")
	} else {
		b.WriteString("The folder's files and project memory were copied. ")
	}
	b.WriteString("Earlier messages mention the old paths: use the new ones from now on, and read a file again before editing it here. ")
	fmt.Fprintf(&b, "Nothing that was running on %s moved: background tasks, dev servers and subagents stopped there, and dependencies such as node_modules were not copied. ", p.From)
	if p.Sidecar != "" {
		fmt.Fprintf(&b, "Transcripts of earlier subagents are in %s/subagents. ", p.Sidecar)
	}
	if p.Note != "" {
		fmt.Fprintf(&b, "Before the handoff you wrote down what was still running: %q. Restart what still needs to run, then ", p.Note)
	} else {
		b.WriteString("Then ")
	}
	fmt.Fprintf(&b, "reply with one short line confirming you're on %s and wait for the next instruction.", p.To)
	return b.String()
}

func syncMemory(r *Remote, localDir, remoteDir string) error {
	entries, err := os.ReadDir(localDir)
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	var files []string
	for _, e := range entries {
		if e.Name() != memoryIndex {
			files = append(files, e.Name())
		}
	}
	if err := r.copy(Archive{Dir: localDir, Paths: files, KeepNewer: true}, remoteDir); err != nil {
		return err
	}
	local, err := os.ReadFile(filepath.Join(localDir, memoryIndex))
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return err
	}
	remote, err := r.sh("if [ -f " + quote(remoteDir+"/"+memoryIndex) + " ]; then cat " + quote(remoteDir+"/"+memoryIndex) + "; fi")
	if err != nil {
		return err
	}
	staging, err := os.MkdirTemp("", "cielago-memory-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)
	if err := os.WriteFile(filepath.Join(staging, memoryIndex), []byte(mergeMemoryIndex(string(local), string(remote))), 0o600); err != nil {
		return err
	}
	return r.copy(Archive{Dir: staging, Paths: []string{memoryIndex}}, remoteDir)
}

const memoryIndex = "MEMORY.md"

func mergeMemoryIndex(incoming, existing string) string {
	key := func(line string) string {
		if _, target, ok := strings.Cut(line, "]("); ok {
			if file, _, ok := strings.Cut(target, ")"); ok {
				return file
			}
		}
		return strings.TrimSpace(line)
	}
	nonEmpty := func(text string) []string {
		var lines []string
		for _, l := range strings.Split(text, "\n") {
			if strings.TrimSpace(l) != "" {
				lines = append(lines, l)
			}
		}
		return lines
	}
	merged := nonEmpty(incoming)
	seen := map[string]bool{}
	for _, l := range merged {
		seen[key(l)] = true
	}
	for _, l := range nonEmpty(existing) {
		if !seen[key(l)] {
			merged = append(merged, l)
			seen[key(l)] = true
		}
	}
	return strings.Join(merged, "\n") + "\n"
}
