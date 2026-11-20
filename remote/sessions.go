package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const maxDiff = 60_000

type machineSessions struct {
	Name     string
	Machine  sh.Machine
	Sessions []liveSession
	Err      error
}

func everySession(ctx context.Context) ([]machineSessions, error) {
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return nil, err
	}
	all := []machineSessions{{Name: local.Name(), Machine: local}}
	cfg, _, err := loadConfig()
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	for _, name := range cfg.Order {
		all = append(all, machineSessions{Name: name})
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
			m.Sessions, m.Err = liveSessions(m.Machine)
			m.Machine = m.Machine.With(ctx)
		})
	}
	wg.Wait()
	return all, nil
}

func describeSessions(all []machineSessions) string {
	var b strings.Builder
	var waiting, unchecked []string
	for _, m := range all {
		if m.Err != nil {
			unchecked = append(unchecked, m.Name)
		}
		for _, s := range m.Sessions {
			if s.Status != "waiting" {
				continue
			}
			waiting = append(waiting, sessionLabel(m.Name, s)+waitingOn(s))
		}
	}
	switch {
	case len(waiting) > 0:
		fmt.Fprintf(&b, "Waiting on you: %s.", strings.Join(waiting, ", "))
	case len(unchecked) == 0:
		fmt.Fprint(&b, "Nothing is waiting on you.")
	default:
		fmt.Fprint(&b, "Nothing that magi could check is waiting on you.")
	}
	if len(unchecked) > 0 {
		fmt.Fprintf(&b, " Not checked: %s.", strings.Join(unchecked, ", "))
	}
	fmt.Fprintln(&b)
	for _, m := range all {
		if m.Err != nil {
			fmt.Fprintf(&b, "%s: not reachable (%v)\n", m.Name, m.Err)
			continue
		}
		if len(m.Sessions) == 0 {
			fmt.Fprintf(&b, "%s: no Claude Code sessions running\n", m.Name)
			continue
		}
		fmt.Fprintf(&b, "%s:\n", m.Name)
		for _, s := range m.Sessions {
			line := fmt.Sprintf("  %s  %s  %s  %s", store.ShortID(s.SessionID), sessionTitle(s), s.Status+waitingOn(s), s.Cwd)
			if s.BridgeSessionID != nil && *s.BridgeSessionID != "" {
				line += "  " + remoteControlLink + *s.BridgeSessionID
			}
			fmt.Fprintln(&b, line)
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

func sessionLabel(machine string, s liveSession) string {
	return sessionTitle(s) + " (" + store.ShortID(s.SessionID) + ") on " + machine
}

func waitingOn(s liveSession) string {
	if s.WaitingFor == "" {
		return ""
	}
	return " (" + s.WaitingFor + ")"
}

func findAnySession(all []machineSessions, query string) (machineSessions, liveSession, error) {
	var matches []string
	var foundOn machineSessions
	var found liveSession
	for _, m := range all {
		for _, s := range m.Sessions {
			if strings.HasPrefix(s.SessionID, query) || len(query) >= 4 && strings.Contains(strings.ToLower(sessionTitle(s)), strings.ToLower(query)) {
				matches = append(matches, sessionLabel(m.Name, s))
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

func waitSession(query string, every time.Duration) (string, error) {
	all, err := everySession(context.Background())
	if err != nil {
		return "", err
	}
	m, s, err := findAnySession(all, query)
	if err != nil {
		return "", err
	}
	for failures := 0; s.Status == "busy"; {
		time.Sleep(every)
		if all, err = everySession(context.Background()); err != nil {
			return "", err
		}
		found := false
		for _, o := range all {
			if _, now, err := findAnySession([]machineSessions{o}, s.SessionID); err == nil && (!found || now.Status == "busy") {
				m, s, found = o, now, true
			}
		}
		if found {
			failures = 0
			continue
		}
		i := slices.IndexFunc(all, func(o machineSessions) bool { return o.Name == m.Name })
		if i < 0 || all[i].Err == nil {
			return sessionLabel(m.Name, s) + " ended.", nil
		}
		if failures++; failures >= waitFailures {
			return "", fmt.Errorf("%s couldn't be checked: %w", m.Name, all[i].Err)
		}
	}
	if s.Status == "waiting" {
		return sessionLabel(m.Name, s) + " is waiting on you" + waitingOn(s) + ".", nil
	}
	return sessionLabel(m.Name, s) + " is " + s.Status + ".", nil
}

func sessionDiff(m sh.Machine, s liveSession) (string, error) {
	out, err := sh.Run(m, sessionScript, "diff", s.Cwd)
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
