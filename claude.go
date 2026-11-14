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

func handoffPrompt(from, to, oldDir, newDir string) string {
	return fmt.Sprintf("[cielago] This session was just handed off from %s to %s. "+
		"The project is now at %s (it was %s on %s); branch, commits, uncommitted changes and secret files were synced. "+
		"Earlier messages mention the old paths: use the new ones from now on, and read a file again before editing it here. "+
		"Reply with one short line confirming you're on %s, then wait for the next instruction.",
		from, to, newDir, oldDir, from, to)
}
