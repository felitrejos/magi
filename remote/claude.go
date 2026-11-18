package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"path/filepath"
	"strconv"
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

type sessionFileSet struct {
	Dir       string
	Files     []string
	Subagents bool
}

func sessionFiles(m Machine, transcriptPath string) (sessionFileSet, error) {
	set := sessionFileSet{Dir: filepath.Dir(transcriptPath)}
	name := filepath.Base(transcriptPath)
	sidecar := strings.TrimSuffix(name, ".jsonl")
	out, err := sh(m, sessionScript, "transcript", transcriptPath)
	if err != nil {
		return set, err
	}
	set.Files = []string{name}
	for _, line := range strings.Fields(string(out)) {
		switch line {
		case "sidecar":
			set.Files = append(set.Files, sidecar)
		case "subagents":
			set.Subagents = true
		}
	}
	return set, nil
}

type liveSession struct {
	PID        int    `json:"pid"`
	SessionID  string `json:"sessionId"`
	Cwd        string `json:"cwd"`
	StartedAt  int64  `json:"startedAt"`
	Status     string `json:"status"`
	WaitingFor string `json:"waitingFor"`
	Name       string `json:"name"`
	fromRegistry
}

type fromRegistry struct {
	BridgeSessionID *string `json:"bridgeSessionId"`
	Entrypoint      string  `json:"entrypoint"`
	ProcStart       any     `json:"procStart"`
}

func (s liveSession) since() string {
	switch v := s.ProcStart.(type) {
	case string:
		return v
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64)
	}
	return ""
}

func liveSessions(m Machine) ([]liveSession, error) {
	out, err := sh(m, sessionScript, "live", m.Claude())
	if err != nil {
		return nil, err
	}
	listed, registry, _ := bytes.Cut(out, []byte{0})
	var sessions []liveSession
	if err := json.Unmarshal(listed, &sessions); err != nil {
		return nil, fmt.Errorf("unreadable claude agents --json on %s: %w", m.Name(), err)
	}
	entries := map[int]fromRegistry{}
	for _, file := range splitNull(registry) {
		var entry struct {
			PID int `json:"pid"`
			fromRegistry
		}
		if json.Unmarshal([]byte(file), &entry) == nil {
			entries[entry.PID] = entry.fromRegistry
		}
	}
	for i := range sessions {
		sessions[i].fromRegistry = entries[sessions[i].PID]
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

func clock(m Machine) (time.Time, error) {
	out, err := sh(m, "date +%s")
	if err != nil {
		return time.Time{}, err
	}
	seconds, err := strconv.ParseInt(strings.TrimSpace(string(out)), 10, 64)
	if err != nil {
		return time.Time{}, fmt.Errorf("unexpected time %q from %s", out, m.Name())
	}
	return time.Unix(seconds, 0), nil
}

func waitForRemoteControl(m Machine, sessionID string, launchedAt time.Time) (string, error) {
	notBefore := launchedAt.Add(-10 * time.Second).UnixMilli()
	for {
		sessions, err := liveSessions(m)
		if err != nil && m.Context().Err() == nil {
			return "", err
		}
		for _, s := range sessions {
			if s.SessionID == sessionID && s.StartedAt >= notBefore && s.BridgeSessionID != nil {
				return *s.BridgeSessionID, nil
			}
		}
		select {
		case <-m.Context().Done():
			return "", fmt.Errorf("the session did not come up with Remote Control on %s in time", m.Name())
		case <-time.After(500 * time.Millisecond):
		}
	}
}

func runningThere(m Machine, sessionID string) (bool, error) {
	_, err := sh(m, sessionScript, "running", sessionID, m.Claude())
	if exitCode(err) == 10 {
		return true, nil
	}
	return false, err
}

func markMoved(m Machine, sessionID, host string) error {
	_, err := sh(m, sessionScript, "moved", "mark", sessionID, host)
	return err
}

func clearMoved(m Machine, sessionID string) error {
	_, err := sh(m, sessionScript, "moved", "clear", sessionID)
	return err
}

func stopSession(m Machine, sessionID string) error {
	_, err := sh(m, sessionScript, "kill", sessionID)
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

const trustScript = `umask 077
f="$HOME/.claude.json"
t="$f.magi-$$"
cat > "$t" || exit 1
if [ "$(cksum < "$f")" != "$1" ]; then
	rm -f "$t"
	echo "it changed while magi was updating it" >&2
	exit 3
fi
mv "$t" "$f"`

func carryTrust(local, remote Machine, localDir, remoteDir string) error {
	trustedHere, err := isTrusted(local, localDir)
	if err != nil || !trustedHere {
		return nil
	}
	if trustedThere, err := isTrusted(remote, remoteDir); err == nil && trustedThere {
		return nil
	}
	if err := markTrusted(remote, remoteDir); err != nil {
		return fmt.Errorf("marking %s trusted there failed, so Claude may ask you to trust it first: %v", remoteDir, err)
	}
	return nil
}

func markTrusted(m Machine, dir string) error {
	out, err := sh(m, `f="$HOME/.claude.json"; cksum < "$f"; cat "$f"`)
	if err != nil {
		return err
	}
	sum, body, found := bytes.Cut(out, []byte("\n"))
	if !found {
		return fmt.Errorf("read ~/.claude.json on %s: no contents", m.Name())
	}
	var state map[string]json.RawMessage
	if err := json.Unmarshal(body, &state); err != nil {
		return fmt.Errorf("read ~/.claude.json on %s: %w", m.Name(), err)
	}
	projects := map[string]map[string]json.RawMessage{}
	if raw, ok := state["projects"]; ok {
		if err := json.Unmarshal(raw, &projects); err != nil {
			return fmt.Errorf("read the projects in ~/.claude.json on %s: %w", m.Name(), err)
		}
	}
	if projects[dir] == nil {
		projects[dir] = map[string]json.RawMessage{}
	}
	projects[dir]["hasTrustDialogAccepted"] = json.RawMessage("true")
	if state["projects"], err = json.Marshal(projects); err != nil {
		return err
	}
	updated, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return err
	}
	_, err = shIn(m, trustScript, updated, string(sum))
	return err
}

type Prompt struct {
	From, To, OldDir, NewDir string
	Git                      bool
	Note                     string
	Sidecar                  string
	Project                  string
	Codex                    bool
	WithMessage              bool
}

func handoffPrompt(p Prompt) string {
	var b strings.Builder
	fmt.Fprintf(&b, "[magi] This session was just handed off from %s to %s. The project is now at %s (it was %s on %s). ", p.From, p.To, p.NewDir, p.OldDir, p.From)
	switch {
	case p.Git && p.Codex:
		b.WriteString("Branch, commits, staged and unstaged changes and secret files were synced. ")
	case p.Git:
		b.WriteString("Branch, commits, staged and unstaged changes, secret files and project memory were synced. ")
	case p.Codex:
		b.WriteString("The folder's files were copied. ")
	default:
		b.WriteString("The folder's files and project memory were copied. ")
	}
	b.WriteString("Earlier messages mention the old paths: use the new ones from now on, and read a file again before editing it here. ")
	fmt.Fprintf(&b, "Nothing that was running on %s moved: background tasks, dev servers and subagents stopped there, and dependencies such as node_modules were not copied. ", p.From)
	if p.Sidecar != "" {
		fmt.Fprintf(&b, "Transcripts of earlier subagents are in %s/subagents. ", p.Sidecar)
	}
	b.WriteString(p.Project)
	if p.Note != "" {
		fmt.Fprintf(&b, "What was still running before the handoff: %q. Restart what still needs to run, then ", p.Note)
	} else {
		b.WriteString("Then ")
	}
	if p.WithMessage {
		b.WriteString("answer the user's message.")
	} else {
		fmt.Fprintf(&b, "reply with one short line confirming you're on %s and wait for the next instruction.", p.To)
	}
	return b.String()
}

func syncMemory(from Machine, fromDir string, to Machine, toDir string) error {
	listing, err := sh(from, memoryScript, "list", fromDir)
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
		out, err := sh(m, memoryScript, "read", dir)
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
	_, err = shIn(to, memoryScript, []byte(mergeMemoryIndex(incoming, existing)), "write", toDir)
	return err
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
