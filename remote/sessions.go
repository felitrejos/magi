package main

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"sync"
)

const maxDiff = 60_000

type machineSessions struct {
	Name     string
	Machine  Machine
	Sessions []liveSession
	Codex    []codexThread
	Err      error
	CodexErr error
}

func everySession(ctx context.Context) ([]machineSessions, error) {
	local, err := newLocal(ctx)
	if err != nil {
		return nil, err
	}
	all := []machineSessions{{Name: local.Name(), Machine: local}}
	cfg, _, err := loadConfig()
	if err == nil {
		for _, name := range cfg.Order {
			all = append(all, machineSessions{Name: name})
		}
	}
	var wg sync.WaitGroup
	for i := range all {
		wg.Go(func() {
			m := &all[i]
			if m.Machine == nil {
				probe, cancel := context.WithTimeout(ctx, reachWait)
				defer cancel()
				r, err := newRemote(probe, cfg.Hosts[m.Name])
				if err != nil {
					m.Err = err
					return
				}
				m.Machine = r
			}
			if m.Sessions, m.Err = liveSessions(m.Machine); m.Err == nil && m.Machine.Codex() != "" {
				m.Codex, m.CodexErr = openCodexSessions(m.Machine)
			}
		})
	}
	wg.Wait()
	return all, nil
}

func describeSessions(all []machineSessions) string {
	var b strings.Builder
	for _, m := range all {
		if m.Err != nil {
			fmt.Fprintf(&b, "%s: not reachable (%v)\n", m.Name, m.Err)
			continue
		}
		if len(m.Sessions) == 0 && len(m.Codex) == 0 && m.CodexErr == nil {
			fmt.Fprintf(&b, "%s: no Claude Code or Codex sessions running\n", m.Name)
			continue
		}
		fmt.Fprintf(&b, "%s:\n", m.Name)
		for _, s := range m.Sessions {
			state := s.Status
			if s.WaitingFor != "" {
				state += " (" + s.WaitingFor + ")"
			}
			line := fmt.Sprintf("  %s  %s  %s  %s", shortID(s.SessionID), sessionTitle(s), state, s.Cwd)
			if s.BridgeSessionID != nil && *s.BridgeSessionID != "" {
				line += "  https://claude.ai/code/" + *s.BridgeSessionID
			}
			fmt.Fprintln(&b, line)
		}
		for _, t := range m.Codex {
			title := t.Name
			if title == "" {
				title = baseName(t.Cwd)
			}
			fmt.Fprintf(&b, "  %s  %s  codex  %s\n", shortID(t.ID), title, t.Cwd)
		}
		if m.CodexErr != nil {
			fmt.Fprintf(&b, "  couldn't read its Codex sessions (%v)\n", m.CodexErr)
		}
	}
	return b.String()
}

func sessionTitle(s liveSession) string {
	if s.Name != "" {
		return s.Name
	}
	return baseName(s.Cwd)
}

func findAnySession(all []machineSessions, query string) (machineSessions, liveSession, error) {
	var matches []string
	var foundOn machineSessions
	var found liveSession
	for _, m := range all {
		for _, s := range m.Sessions {
			if strings.HasPrefix(s.SessionID, query) || len(query) >= 4 && strings.Contains(strings.ToLower(sessionTitle(s)), strings.ToLower(query)) {
				matches = append(matches, fmt.Sprintf("%s %s on %s", shortID(s.SessionID), sessionTitle(s), m.Name))
				foundOn, found = m, s
			}
		}
	}
	switch len(matches) {
	case 0:
		return foundOn, found, fmt.Errorf("no running Claude Code session matches %q", query)
	case 1:
		return foundOn, found, nil
	}
	slices.Sort(matches)
	return foundOn, found, fmt.Errorf("%q matches more than one session: %s", query, strings.Join(matches, ", "))
}

func sessionDiff(m Machine, s liveSession) (string, error) {
	out, err := sh(m, sessionScript, "diff", s.Cwd)
	if err != nil {
		return "", err
	}
	text := string(out)
	if strings.TrimSpace(text) == "" {
		return "No uncommitted changes in " + s.Cwd, nil
	}
	if len(text) > maxDiff {
		text = "…(earlier part left out)\n" + strings.ToValidUTF8(text[len(text)-maxDiff:], "")
	}
	return text, nil
}
