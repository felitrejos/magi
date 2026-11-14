package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"time"
)

type logEntry struct {
	Time    time.Time `json:"time"`
	Kind    string    `json:"kind"`
	Session string    `json:"session"`
	From    string    `json:"from"`
	To      string    `json:"to"`
	FromDir string    `json:"from_dir"`
	ToDir   string    `json:"to_dir"`
}

func logPath() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".cielago", "handoffs.jsonl"), nil
}

func appendLog(e logEntry) error {
	path, err := logPath()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer f.Close()
	return json.NewEncoder(f).Encode(e)
}

func readLog() ([]logEntry, error) {
	path, err := logPath()
	if err != nil {
		return nil, err
	}
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var entries []logEntry
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		var e logEntry
		if json.Unmarshal(scanner.Bytes(), &e) == nil {
			entries = append(entries, e)
		}
	}
	return entries, scanner.Err()
}

func lastHandoffTo(host string) (string, error) {
	entries, err := readLog()
	if err != nil {
		return "", err
	}
	for _, e := range slices.Backward(entries) {
		if e.Kind == "push" && e.To == host {
			return e.Session, nil
		}
	}
	return "", fmt.Errorf("no handoff to %s is recorded here; pass the session id: cielago pull %s <session-id>", host, host)
}

func shortID(id string) string {
	return id[:min(8, len(id))]
}

func status(w io.Writer) error {
	entries, err := readLog()
	if err != nil {
		return err
	}
	if len(entries) == 0 {
		fmt.Fprintln(w, "No handoffs yet.")
		return nil
	}
	fmt.Fprintln(w, "Recent handoffs:")
	for _, e := range slices.Backward(entries[max(0, len(entries)-15):]) {
		fmt.Fprintf(w, "  %s  %-4s  %s  %s:%s → %s:%s\n", e.Time.Local().Format("2006-01-02 15:04"), e.Kind, shortID(e.Session), e.From, e.FromDir, e.To, e.ToDir)
	}
	local, err := newLocal(context.Background())
	if err != nil {
		return err
	}
	checked := map[string]bool{}
	var parked []string
	for _, e := range entries {
		if e.Kind != "push" || e.From != local.Name() || checked[e.FromDir] {
			continue
		}
		checked[e.FromDir] = true
		if ref, err := sh(local, "git -C "+quote(e.FromDir)+" rev-parse -q --verify "+parkedRef+" 2>/dev/null || :"); err == nil && len(ref) > 0 {
			parked = append(parked, e.FromDir)
		}
	}
	if len(parked) == 0 {
		return nil
	}
	fmt.Fprintln(w, "\nParked checkouts here (a session left them; it can come back, and they're refused if edited meanwhile):")
	for _, dir := range parked {
		fmt.Fprintf(w, "  %s    unpark with: cielago unpark %s\n", dir, quote(dir))
	}
	return nil
}

func unpark(dir string, out io.Writer) error {
	local, err := newLocal(context.Background())
	if err != nil {
		return err
	}
	if _, err := sh(local, "cd "+quote(dir)+" && git update-ref -d "+parkedRef); err != nil {
		return err
	}
	fmt.Fprintf(out, "%s is no longer parked.\n", dir)
	return nil
}
