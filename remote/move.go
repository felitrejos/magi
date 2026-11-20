package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"slices"
	"strconv"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

type mover struct {
	session string
	host    string
	out     io.Writer
}

func busyError(session string) error {
	id := store.ShortID(session)
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
	if s.Live == nil {
		return m.now(local, host, s, false)
	}
	_, err = sh.Run(local, sessionScript, "stop", strconv.Itoa(s.Live.PID), s.ID, s.Live.since(), local.Claude())
	if sh.ExitCode(err) == sh.ExitBusy {
		return busyError(s.ID)
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
	return s, fmt.Errorf("no Claude Code session %s on %s", id, local.Name())
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
	res, err := Move{Kind: "push", HostName: m.host, Host: host, Session: s, Stopped: stopped, Out: m.out, Progress: progress, Retire: closeHere}.Run(context.Background())
	if err != nil {
		return err
	}
	fmt.Fprintln(m.out, res.Report())
	return nil
}
