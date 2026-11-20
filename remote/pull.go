package main

import (
	"context"
	"encoding/json"
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

const (
	pullWait   = 30 * time.Minute
	pullBudget = 3 * time.Minute
)

var turnEndPoll = 2 * time.Second

var errNoSession = errors.New("no session")

type remoteSession struct {
	ID         string
	Transcript string
	Cwd        string
	Live       *liveSession
}

func pullHere(hostName, sessionID string, out io.Writer, open bool) (string, error) {
	host, err := loadHost(hostName)
	if err != nil {
		return "", err
	}
	unlock, err := lockMove(sessionID)
	if err != nil {
		return "", err
	}
	defer unlock()
	if err := recoverBefore(sessionID, false); err != nil {
		return "", err
	}
	res, err := Move{Kind: "pull", HostName: hostName, Host: host, Session: remoteSession{ID: sessionID}, NoTab: !open, Out: out}.Run(context.Background())
	if err != nil {
		return "", err
	}
	return res.Report(), nil
}

func findSession(r sh.Machine, id string) (remoteSession, error) {
	if !store.SessionIDPattern.MatchString(id) {
		return remoteSession{}, fmt.Errorf("%q is not a session id", id)
	}
	s := remoteSession{ID: id}
	out, err := sh.Run(r, sessionScript, "find", id)
	if err != nil {
		return s, err
	}
	if s.Transcript = strings.TrimSpace(string(out)); s.Transcript == "" {
		return s, fmt.Errorf("%w %s on %s", errNoSession, id, r.Name())
	}
	sessions, err := liveSessions(r)
	if err != nil {
		return s, err
	}
	for i := range sessions {
		if sessions[i].SessionID == id && (s.Live == nil || sessions[i].StartedAt > s.Live.StartedAt) {
			s.Live = &sessions[i]
		}
	}
	if s.Live != nil {
		s.Cwd = s.Live.Cwd
		return s, nil
	}
	out, err = sh.Run(r, sessionScript, "cwd", s.Transcript)
	if err != nil {
		return s, err
	}
	for _, line := range slices.Backward(strings.Split(string(out), "\n")) {
		var record struct {
			Cwd string `json:"cwd"`
		}
		if json.Unmarshal([]byte(line), &record) == nil && record.Cwd != "" {
			s.Cwd = record.Cwd
			return s, nil
		}
	}
	return s, fmt.Errorf("couldn't tell which folder session %s on %s ran in", id, r.Name())
}

func stopAtTurnEnd(r *sh.Remote, s remoteSession, out io.Writer) error {
	if s.Live == nil {
		return nil
	}
	pid := strconv.Itoa(s.Live.PID)
	told := false
	for {
		_, err := sh.Run(r, sessionScript, "stop", pid, s.ID, s.Live.since(), r.Claude())
		if sh.ExitCode(err) != sh.ExitBusy {
			return err
		}
		if !told {
			fmt.Fprintf(out, "Waiting for the session on %s to finish its turn...\n", r.Name())
			told = true
		}
		select {
		case <-r.Context().Done():
			return fmt.Errorf("the session on %s was still busy", r.Name())
		case <-time.After(turnEndPoll):
		}
	}
}

func resumeCommand(program, sessionID, name, prompt string) []string {
	return []string{program, "--resume", sessionID, "--remote-control", name, prompt}
}
