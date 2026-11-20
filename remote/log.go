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
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
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

func status(w io.Writer) error {
	if err := reportMoves(w); err != nil {
		return err
	}
	if err := reportWanted(w); err != nil {
		return err
	}
	entries, err := readLog()
	if err != nil {
		return err
	}
	if err := reportForwards(w); err != nil {
		return err
	}
	if len(entries) == 0 {
		fmt.Fprintln(w, "No handoffs yet.")
		return nil
	}
	fmt.Fprintln(w, "Recent handoffs:")
	for _, e := range slices.Backward(entries[max(0, len(entries)-15):]) {
		if e.Note != "" {
			fmt.Fprintf(w, "  %s  %s\n", e.Time.Local().Format("2006-01-02 15:04"), e.Note)
			continue
		}
		fmt.Fprintf(w, "  %s  %-4s  %s  %s:%s → %s:%s\n", e.Time.Local().Format("2006-01-02 15:04"), e.Kind, e.Session, e.From, e.FromDir, e.To, e.ToDir)
	}
	local, err := sh.NewLocal(context.Background())
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
		if ref, err := sh.Run(local, `git -C "$1" rev-parse -q --verify "$2" 2>/dev/null || :`, e.FromDir, parkedRef); err == nil && len(ref) > 0 {
			parked = append(parked, e.FromDir)
		}
	}
	if len(parked) == 0 {
		return nil
	}
	fmt.Fprintln(w, "\nParked checkouts here. A session left each one and can come back, and then its changes replace the uncommitted ones left here, unless they were edited meanwhile. To make a checkout's uncommitted changes count as the user's own, call unpark with its folder:")
	for _, dir := range parked {
		fmt.Fprintf(w, "  %s\n", dir)
	}
	return nil
}

func unparkRepo(m sh.Machine, repo string) error {
	if err := deleteRefs(m, repo, parkedRef); err != nil {
		return err
	}
	links, err := gitlinks(m, repo)
	if err != nil {
		return err
	}
	for _, link := range links {
		if err := unparkRepo(m, repo+"/"+link); err != nil {
			return err
		}
	}
	return nil
}

func unpark(dir string, out io.Writer) error {
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return err
	}
	top, err := sh.Run(local, `git -C "$1" rev-parse --show-toplevel`, dir)
	if err != nil {
		return fmt.Errorf("%s isn't a git checkout, and only git checkouts are parked: %w", dir, err)
	}
	if err := unparkRepo(local, strings.TrimSpace(string(top))); err != nil {
		return err
	}
	fmt.Fprintf(out, "%s is no longer parked.\n", dir)
	return nil
}
