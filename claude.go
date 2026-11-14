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

func sessionFiles(m Machine, transcriptPath string) (dir string, files []string, err error) {
	dir = filepath.Dir(transcriptPath)
	name := filepath.Base(transcriptPath)
	sidecar := strings.TrimSuffix(name, ".jsonl")
	out, err := sh(m, "cd "+quote(dir)+" && [ -f "+quote(name)+" ] && if [ -d "+quote(sidecar)+" ]; then echo sidecar; fi")
	if err != nil {
		return "", nil, err
	}
	files = []string{name}
	if strings.TrimSpace(string(out)) == "sidecar" {
		files = append(files, sidecar)
	}
	return dir, files, nil
}

type liveSession struct {
	PID             int     `json:"pid"`
	SessionID       string  `json:"sessionId"`
	Cwd             string  `json:"cwd"`
	Status          string  `json:"status"`
	StartedAt       int64   `json:"startedAt"`
	BridgeSessionID *string `json:"bridgeSessionId"`
}

func liveSessions(m Machine) ([]liveSession, error) {
	out, err := sh(m, `for f in "$HOME"/.claude/sessions/*.json; do [ -f "$f" ] && cat "$f" && printf '\0'; done; true`)
	if err != nil {
		return nil, err
	}
	var sessions []liveSession
	for _, file := range splitNull(out) {
		var s liveSession
		if json.Unmarshal([]byte(file), &s) == nil {
			sessions = append(sessions, s)
		}
	}
	return sessions, nil
}

func splitNull(out []byte) []string {
	var items []string
	for _, item := range strings.Split(string(out), "\x00") {
		if item != "" {
			items = append(items, item)
		}
	}
	return items
}

func waitForRemoteControl(m Machine, sessionID string, launchedAt time.Time) error {
	notBefore := launchedAt.Add(-10 * time.Second).UnixMilli()
	for {
		sessions, err := liveSessions(m)
		if err != nil {
			return err
		}
		for _, s := range sessions {
			if s.SessionID == sessionID && s.StartedAt >= notBefore && s.BridgeSessionID != nil {
				return nil
			}
		}
		select {
		case <-m.Context().Done():
			return fmt.Errorf("the session did not come up with Remote Control on %s in time", m.Name())
		case <-time.After(500 * time.Millisecond):
		}
	}
}

func stopSession(m Machine, sessionID string) error {
	_, err := sh(m, "pkill -TERM -f -- "+quote("[-]-resume "+sessionID)+"; true")
	return err
}

func isTrusted(m Machine, dir string) (bool, error) {
	out, err := sh(m, `cat "$HOME"/.claude.json`)
	if err != nil {
		return false, err
	}
	var state struct {
		Projects map[string]struct {
			Trusted bool `json:"hasTrustDialogAccepted"`
		} `json:"projects"`
	}
	if err := json.Unmarshal(out, &state); err != nil {
		return false, fmt.Errorf("read ~/.claude.json on %s: %w", m.Name(), err)
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

func syncMemory(local *Local, from Machine, fromDir string, to Machine, toDir string) error {
	listing, err := sh(from, "if [ -d "+quote(fromDir)+" ]; then cd "+quote(fromDir)+` && for f in * .[!.]*; do [ -e "$f" ] && printf '%s\0' "$f"; done; fi; true`)
	if err != nil {
		return err
	}
	var files []string
	hasIndex := false
	for _, f := range splitNull(listing) {
		if f == memoryIndex {
			hasIndex = true
			continue
		}
		files = append(files, f)
	}
	if err := transfer(from, fromDir, Archive{Paths: files, KeepNewer: true}, to, toDir); err != nil {
		return err
	}
	if !hasIndex {
		return nil
	}
	read := func(m Machine, dir string) (string, error) {
		path := quote(dir + "/" + memoryIndex)
		out, err := sh(m, "if [ -f "+path+" ]; then cat "+path+"; fi")
		return string(out), err
	}
	incoming, err := read(from, fromDir)
	if err != nil {
		return err
	}
	existing, err := read(to, toDir)
	if err != nil {
		return err
	}
	staging, err := os.MkdirTemp("", "cielago-memory-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(staging)
	if err := os.WriteFile(filepath.Join(staging, memoryIndex), []byte(mergeMemoryIndex(incoming, existing)), 0o600); err != nil {
		return err
	}
	return transfer(local, staging, Archive{Paths: []string{memoryIndex}}, to, toDir)
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
