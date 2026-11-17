package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/BurntSushi/toml"
)

func codexTabLabel(name string) string {
	return name + " · codex"
}

func codexResume(sessionID, prompt string) []string {
	return []string{"-c", "check_for_update_on_startup=false", "-c", `tui.resume_cwd="current"`, "resume", sessionID, prompt}
}

func resumeHint(s remoteSession) string {
	if s.Codex {
		return "codex resume " + s.ID
	}
	return "claude --resume " + s.ID
}

func findCodexSession(m Machine, id string) (remoteSession, error) {
	s := remoteSession{ID: id, Codex: true}
	out, err := sh(m, codexScript, "find", id)
	if err != nil {
		return s, err
	}
	sessions, rollout, _ := strings.Cut(strings.TrimRight(string(out), "\n"), "\n")
	if rollout == "" {
		return s, fmt.Errorf("no session %s on %s", id, m.Name())
	}
	if !strings.HasSuffix(rollout, ".jsonl") {
		return s, fmt.Errorf("session %s on %s is a compressed Codex session (%s), and magi moves only uncompressed ones", shortID(id), m.Name(), rollout)
	}
	var ok bool
	if s.Rollout, ok = strings.CutPrefix(rollout, sessions+"/"); !ok {
		return s, fmt.Errorf("session %s on %s is at %s, outside %s", shortID(id), m.Name(), rollout, sessions)
	}
	s.Transcript = rollout
	out, err = sh(m, codexScript, "cwd", rollout)
	if err != nil {
		return s, err
	}
	var record struct {
		Payload struct {
			Cwd string `json:"cwd"`
		} `json:"payload"`
	}
	if json.Unmarshal(out, &record) != nil || record.Payload.Cwd == "" {
		return s, fmt.Errorf("couldn't tell which folder session %s on %s ran in", id, m.Name())
	}
	s.Cwd = record.Payload.Cwd
	out, err = sh(m, codexScript, "running", id, rollout, s.Cwd)
	if err != nil {
		return s, err
	}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		pid, kind, _ := strings.Cut(line, " ")
		switch kind {
		case "session":
			n, err := strconv.Atoi(pid)
			if err != nil {
				return s, fmt.Errorf("unexpected process %q running session %s on %s", line, shortID(id), m.Name())
			}
			s.Running = append(s.Running, n)
		case "folder":
			s.Unsure = append(s.Unsure, pid)
		}
	}
	if len(s.Running) > 0 {
		s.StaleTurn, err = staleTurn(m, s)
	}
	return s, err
}

func staleTurn(m Machine, s remoteSession) (string, error) {
	out, err := sh(m, codexScript, "turn", s.Transcript)
	if err != nil || len(bytes.TrimSpace(out)) == 0 {
		return "", err
	}
	var event struct {
		Timestamp string `json:"timestamp"`
		Payload   struct {
			Type string `json:"type"`
		} `json:"payload"`
	}
	if err := json.Unmarshal(out, &event); err != nil {
		return "", fmt.Errorf("unreadable turn in %s on %s: %w", s.Transcript, m.Name(), err)
	}
	if event.Payload.Type != "task_started" {
		return "", nil
	}
	began, err := time.Parse(time.RFC3339Nano, event.Timestamp)
	if err != nil {
		return "", fmt.Errorf("unreadable turn time %q in %s on %s: %w", event.Timestamp, s.Transcript, m.Name(), err)
	}
	args := []string{"started"}
	for _, pid := range s.Running {
		args = append(args, strconv.Itoa(pid))
	}
	out, err = sh(m, codexScript, args...)
	if err != nil {
		return "", err
	}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		pid, start, _ := strings.Cut(line, " ")
		secs, err := strconv.ParseInt(start, 10, 64)
		if err != nil {
			return "", fmt.Errorf("unreadable start time %q of process %s on %s: %w", start, pid, m.Name(), err)
		}
		if !time.Unix(secs, 0).After(began.Truncate(time.Second)) {
			return "", nil
		}
	}
	return event.Timestamp, nil
}

func refuseOpenCodex(m Machine, id string, stopping []int) error {
	out, err := sh(m, codexScript, "open", id)
	if err != nil {
		return err
	}
	for line := range strings.Lines(string(out)) {
		pid, args, _ := strings.Cut(strings.TrimSpace(line), " ")
		if slices.ContainsFunc(stopping, func(running int) bool { return strconv.Itoa(running) == pid }) {
			continue
		}
		if strings.Contains(args, " app-server") {
			return fmt.Errorf("session %s is still open in the Codex background server on %s (pid %s); it lets go once it has been idle for a while, or run codex app-server daemon restart there (which interrupts other Codex work on it), then try again", shortID(id), m.Name(), pid)
		}
		return fmt.Errorf("session %s is open in Codex on %s (pid %s); quit it there, then try again", shortID(id), m.Name(), pid)
	}
	return nil
}

func (s remoteSession) refuseUnsure(m Machine) error {
	if len(s.Unsure) == 0 {
		return nil
	}
	return fmt.Errorf("a Codex is open in %s on %s (pid %s), and magi can't tell whether it's session %s; quit Codex there, then move it again", s.Cwd, m.Name(), strings.Join(s.Unsure, ", "), shortID(s.ID))
}

func stopCodex(m Machine, s remoteSession) error {
	args := []string{"stop", s.Transcript, s.StaleTurn}
	for _, pid := range s.Running {
		args = append(args, strconv.Itoa(pid))
	}
	_, err := sh(m, codexScript, args...)
	return err
}

func moveRollout(from Machine, path, rel, fromRoot string, to Machine, toRoot string) (string, int, error) {
	data, err := sh(from, `cat -- "$1"`, path)
	if err != nil {
		return "", 0, err
	}
	rewritten, err := rewriteRollout(data, fromRoot, toRoot)
	if err != nil {
		return "", 0, err
	}
	out, err := shIn(to, codexScript, rewritten, "write", rel)
	if err != nil {
		return "", 0, err
	}
	return strings.TrimSpace(string(out)), len(rewritten), nil
}

func rewriteRollout(data []byte, from, to string) ([]byte, error) {
	if from == to {
		return data, nil
	}
	old, err := jsonText(from)
	if err != nil {
		return nil, err
	}
	replacement, err := jsonText(to)
	if err != nil {
		return nil, err
	}
	lines := bytes.SplitAfter(data, []byte("\n"))
	for i, line := range lines {
		head := line
		if j := bytes.IndexByte(line[min(1, len(line)):], '{'); j >= 0 {
			head = line[:j+1]
		}
		if !bytes.Contains(head, []byte(`"type":"response_item"`)) {
			lines[i] = replacePath(line, `"`+old, `"`+replacement)
		}
	}
	return bytes.Join(lines, nil), nil
}

func jsonText(s string) (string, error) {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(s); err != nil {
		return "", err
	}
	return strings.TrimSuffix(strings.TrimPrefix(strings.TrimSuffix(b.String(), "\n"), `"`), `"`), nil
}

func replacePath(line []byte, old, replacement string) []byte {
	var b bytes.Buffer
	for {
		j := bytes.Index(line, []byte(old))
		if j < 0 {
			b.Write(line)
			return b.Bytes()
		}
		end := j + len(old)
		b.Write(line[:j])
		if (j == 0 || line[j-1] != '\\') && end < len(line) && (line[end] == '"' || line[end] == '/') {
			b.WriteString(replacement)
		} else {
			b.Write(line[j:end])
		}
		line = line[end:]
	}
}

func waitForCodex(m Machine, rollout string, written int) error {
	for {
		out, err := sh(m, codexScript, "size", rollout)
		if err != nil {
			return err
		}
		size, err := strconv.Atoi(strings.TrimSpace(string(out)))
		if err != nil {
			return fmt.Errorf("unexpected size %q of %s on %s", out, rollout, m.Name())
		}
		if size > written {
			return nil
		}
		select {
		case <-m.Context().Done():
			return fmt.Errorf("the Codex session did not start on %s in time", m.Name())
		case <-time.After(500 * time.Millisecond):
		}
	}
}

func stopCodexSession(m Machine, sessionID string) error {
	_, err := sh(m, codexScript, "kill", sessionID)
	return err
}

type codexConfig struct {
	Projects map[string]struct {
		TrustLevel string `toml:"trust_level"`
	} `toml:"projects"`
}

func readCodexConfig(m Machine) (string, codexConfig, error) {
	var cfg codexConfig
	out, err := sh(m, codexScript, "config")
	if err != nil {
		return "", cfg, err
	}
	sum, body, _ := strings.Cut(string(out), "\n")
	if _, err := toml.Decode(body, &cfg); err != nil {
		return "", cfg, fmt.Errorf("read Codex's config.toml on %s: %w", m.Name(), err)
	}
	return sum, cfg, nil
}

func (c codexConfig) trusts(dir string) bool {
	for d := dir; ; d = filepath.Dir(d) {
		if c.Projects[d].TrustLevel == "trusted" {
			return true
		}
		if d == filepath.Dir(d) {
			return false
		}
	}
}

func carryCodexTrust(local, remote Machine, localDir, remoteDir, remoteRoot string) error {
	_, here, err := readCodexConfig(local)
	if err != nil {
		return err
	}
	if !here.trusts(localDir) {
		return nil
	}
	sum, there, err := readCodexConfig(remote)
	if err != nil {
		return err
	}
	if there.trusts(remoteDir) {
		return nil
	}
	if _, ok := there.Projects[remoteRoot]; ok {
		return fmt.Errorf("the Codex settings on %s don't trust %s, so it may ask you to trust it first", remote.Name(), remoteRoot)
	}
	entry := fmt.Sprintf("\n[projects.%s]\ntrust_level = \"trusted\"\n", tomlKey(remoteRoot))
	if _, err := shIn(remote, codexScript, []byte(entry), "trust", sum); err != nil {
		return fmt.Errorf("marking %s trusted for Codex there failed, so Codex may ask you to trust it first: %v", remoteRoot, err)
	}
	return nil
}
