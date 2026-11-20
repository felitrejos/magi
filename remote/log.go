package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"time"

	"github.com/felitrejos/magi/remote/internal/store"
)

type logEntry struct {
	Time    time.Time `json:"time"`
	Kind    string    `json:"kind"`
	Session string    `json:"session"`
	From    string    `json:"from"`
	To      string    `json:"to"`
	FromDir string    `json:"from_dir"`
	ToDir   string    `json:"to_dir"`
	Note    string    `json:"note,omitempty"`
	Bytes   int64     `json:"bytes,omitzero"`
	Seconds float64   `json:"seconds,omitzero"`
}

func appendLog(e logEntry) error {
	path, err := store.Dir("handoffs.jsonl")
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
	return errors.Join(json.NewEncoder(f).Encode(e), f.Close())
}

func readLog() ([]logEntry, error) {
	path, err := store.Dir("handoffs.jsonl")
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
