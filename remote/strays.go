package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const strayCleanup = 20 * time.Second

func strayDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".magi", "strays"), nil
}

func saveStray(sessionID string, s Stray) error {
	dir, err := strayDir()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(s)
	if err != nil {
		return err
	}
	path := filepath.Join(dir, sessionID+".json")
	if err := os.WriteFile(path+".tmp", data, 0o600); err != nil {
		return err
	}
	return os.Rename(path+".tmp", path)
}

func loadStrays() (map[string]Stray, error) {
	dir, err := strayDir()
	if err != nil {
		return nil, err
	}
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil {
		return nil, err
	}
	strays := map[string]Stray{}
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		var s Stray
		if err := json.Unmarshal(data, &s); err != nil {
			return nil, fmt.Errorf("%s: %w", path, err)
		}
		strays[strings.TrimSuffix(filepath.Base(path), ".json")] = s
	}
	return strays, nil
}

func cleanStray(sessionID string, s Stray) error {
	host, err := loadHost(s.Host)
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), strayCleanup)
	defer cancel()
	remote, err := newRemote(ctx, host)
	if err != nil {
		return err
	}
	stop := stopSession
	if s.Codex {
		stop = stopCodexSession
	}
	if err := stop(remote, sessionID); err != nil {
		return err
	}
	if err := newLauncher(host, remote).Discard(s.Tab); err != nil && !tabGone(err) {
		return err
	}
	dir, err := strayDir()
	if err != nil {
		return err
	}
	return os.Remove(filepath.Join(dir, sessionID+".json"))
}

func cleanStrayFor(sessionID string) error {
	dir, err := strayDir()
	if err != nil {
		return err
	}
	data, err := os.ReadFile(filepath.Join(dir, sessionID+".json"))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	var s Stray
	if err := json.Unmarshal(data, &s); err != nil {
		return fmt.Errorf("the record of an earlier failed handoff of this session can't be read (%w); remove %s", err, filepath.Join(dir, sessionID+".json"))
	}
	if err := cleanStray(sessionID, s); err != nil {
		return fmt.Errorf("a copy of this session left on %s by an earlier handoff couldn't be stopped: %w", s.Host, err)
	}
	return nil
}

func reportStrays(w io.Writer) error {
	strays, err := loadStrays()
	if err != nil || len(strays) == 0 {
		return err
	}
	fmt.Fprintln(w, "\nCopies left running by a handoff that failed partway:")
	for id, s := range strays {
		if err := cleanStray(id, s); err != nil {
			fmt.Fprintf(w, "  %s on %s is still there (%v); stop it there or run magi status again\n", shortID(id), s.Host, err)
			continue
		}
		fmt.Fprintf(w, "  %s on %s, stopped now\n", shortID(id), s.Host)
	}
	return nil
}
