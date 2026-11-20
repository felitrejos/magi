package main

import (
	"context"
	"fmt"
	"slices"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func status() (string, error) {
	var w strings.Builder
	if err := reportMoves(&w); err != nil {
		return "", err
	}
	if err := reportWanted(&w); err != nil {
		return "", err
	}
	entries, err := readLog()
	if err != nil {
		return "", err
	}
	if err := reportForwards(&w); err != nil {
		return "", err
	}
	if len(entries) == 0 {
		fmt.Fprintln(&w, "No handoffs yet.")
		return w.String(), nil
	}
	fmt.Fprintln(&w, "Recent handoffs:")
	for _, e := range slices.Backward(entries[max(0, len(entries)-15):]) {
		if e.Note != "" {
			fmt.Fprintf(&w, "  %s  %s\n", e.Time.Local().Format("2006-01-02 15:04"), e.Note)
			continue
		}
		fmt.Fprintf(&w, "  %s  %-4s  %s  %s:%s → %s:%s\n", e.Time.Local().Format("2006-01-02 15:04"), e.Kind, e.Session, e.From, e.FromDir, e.To, e.ToDir)
	}
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return "", err
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
		return w.String(), nil
	}
	fmt.Fprintln(&w, "\nParked checkouts here. A session left each one and can come back, and then its changes replace the uncommitted ones left here, unless they were edited meanwhile. To make a checkout's uncommitted changes count as the user's own, call unpark with its folder:")
	for _, dir := range parked {
		fmt.Fprintf(&w, "  %s\n", dir)
	}
	return w.String(), nil
}

func unpark(dir string) (string, error) {
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return "", err
	}
	top, err := sh.Run(local, `git -C "$1" rev-parse --show-toplevel`, dir)
	if err != nil {
		return "", fmt.Errorf("%s isn't a git checkout, and only git checkouts are parked: %w", dir, err)
	}
	if err := unparkRepo(local, strings.TrimSpace(string(top))); err != nil {
		return "", err
	}
	return dir + " is no longer parked.\n", nil
}
