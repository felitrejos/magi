package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

type mover struct {
	session string
	host    string
	out     io.Writer
}

func busyError(s remoteSession) error {
	id := store.ShortID(s.ID)
	return fmt.Errorf("session %s is in the middle of a turn or waiting on the user, so it wasn't moved; run magi wait %s as a background shell command, then move it if that says it's idle, or ask the user to answer it first if that says it's waiting on them", id, id)
}

func (m mover) run() error {
	if !store.SessionIDPattern.MatchString(m.session) {
		return fmt.Errorf("%q is not a session id", m.session)
	}
	host, err := loadHost(m.host)
	if err != nil {
		return err
	}
	unlock, err := lockMove(m.session)
	if err != nil {
		return err
	}
	defer unlock()
	if err := recoverBefore(m.session, true); err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return err
	}
	s, err := findLocalSession(local, m.session)
	if err != nil {
		return err
	}
	if s.Codex {
		return m.codex(local, host, s)
	}
	if s.Live == nil {
		return m.now(local, host, s, false)
	}
	_, err = sh.Run(local, sessionScript, "stop", strconv.Itoa(s.Live.PID), s.ID, s.Live.since(), local.Claude())
	if sh.ExitCode(err) == sh.ExitBusy {
		return busyError(s)
	}
	if err != nil {
		return err
	}
	return m.now(local, host, s, true)
}

func (m mover) codex(local *sh.Local, host Host, s remoteSession) error {
	if host.Codex == "" {
		return fmt.Errorf("session %s is a Codex session, and %s isn't set up for Codex; call setup_machine for %s with codex set to true", store.ShortID(s.ID), m.host, m.host)
	}
	if err := s.refuseUnsure(local); err != nil {
		return err
	}
	if err := refuseOpenCodex(local, s.ID, s.Running); err != nil {
		return err
	}
	remote, err := newRemote(local.Context(), host)
	if err != nil {
		return err
	}
	if err := refuseOpenCodex(remote, s.ID, nil); err != nil {
		return err
	}
	if len(s.Running) == 0 {
		return m.now(local, host, s, false)
	}
	err = stopCodex(local, s)
	if sh.ExitCode(err) == sh.ExitBusy {
		return busyError(s)
	}
	if err != nil {
		return err
	}
	return m.now(local, host, s, true)
}

func lockMove(sessionID string) (func(), error) {
	path, err := store.SessionPath("armed", sessionID, ".lock")
	if err != nil {
		return nil, err
	}
	return store.Lock(path, fmt.Errorf("session %s is already being moved", store.ShortID(sessionID)))
}

func findLocalSession(local *sh.Local, id string) (remoteSession, error) {
	s, err := findSession(local, id)
	if !errors.Is(err, errNoSession) {
		return s, err
	}
	if live, _ := liveSessions(local); slices.ContainsFunc(live, func(l liveSession) bool { return l.SessionID == id }) {
		return s, fmt.Errorf("session %s hasn't started a conversation yet, so there's nothing to move", store.ShortID(id))
	}
	return s, fmt.Errorf("no Claude Code or Codex session %s on %s", id, local.Name())
}

func (m mover) now(local *sh.Local, host Host, s remoteSession, stopped bool) error {
	progress := func(step string) {
		fmt.Fprintf(m.out, "Moving session %s to %s: %s\n", store.ShortID(s.ID), m.host, step)
	}
	closeHere := func() (int, error) {
		finish, cancel := context.WithTimeout(context.Background(), finishBudget)
		defer cancel()
		return retire(local.With(finish), m.host, s.ID, s.Transcript, 0)
	}
	show := func(block string) { fmt.Fprintln(m.out, block) }
	res, err := Handoff{HostName: m.host, Host: host, SessionID: s.ID, TranscriptPath: s.Transcript, Cwd: s.Cwd, Progress: progress, Show: show, Codex: s.Codex, Rollout: s.Rollout, Retire: closeHere}.Run(context.Background())
	if err != nil && stopped {
		program := local.Claude()
		if s.Codex {
			program = local.Codex()
		}
		return reopen(local, "here", program, localGroup(s.Cwd, host.LocalRoots), s, "Moving this session to "+m.host, err)
	}
	if err != nil {
		return err
	}
	fmt.Fprintln(m.out, movedReport(s.ID, m.host, res))
	return nil
}

func movedReport(sessionID, host string, res Result) string {
	where := "running " + res.Where
	if len(res.ShownOn) > 0 {
		where = "open in " + strings.Join(res.ShownOn, ", ")
	}
	msg := fmt.Sprintf("Moved session %s to %s in %s. It's %s", store.ShortID(sessionID), host, res.Summary(), where)
	if res.NewFolder != "" {
		msg += fmt.Sprintf(". %s didn't have the project yet, so it now lives at %s", host, res.NewFolder)
	}
	if forwarded := res.Forwarded(); forwarded != "" {
		msg += ". " + forwarded
	}
	if len(res.Notes) > 0 {
		msg += ". Notes: " + res.Noted()
	}
	return msg + "."
}
