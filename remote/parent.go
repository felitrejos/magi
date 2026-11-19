package main

import (
	"context"
	"os"
	"slices"
	"strings"
)

const parentEnv = "MAGI_PARENT"

type parentSession struct {
	ID   string
	Name string
}

func delegatingSession(c caller, project string) (*parentSession, error) {
	if c.PID == 0 {
		return nil, nil
	}
	local, err := newLocal(context.Background())
	if err != nil {
		return nil, err
	}
	sessions, err := liveSessions(local)
	if err != nil {
		return nil, err
	}
	i := slices.IndexFunc(sessions, func(s liveSession) bool { return s.PID == c.PID })
	if i < 0 {
		return nil, nil
	}
	live := sessions[i]
	name := strings.TrimSpace(live.Name)
	if name == "" {
		name = project
	}
	return &parentSession{ID: live.SessionID, Name: name}, nil
}

func (p *parentSession) tabLabel() string {
	return machineLabel() + " ← " + p.Name
}

func withParent(id string, command []string) []string {
	if id == "" {
		return command
	}
	return append([]string{"env", parentEnv + "=" + id}, command...)
}

func machineLabel() string {
	name, _ := os.Hostname()
	short, _, _ := strings.Cut(name, ".")
	return strings.ToLower(short)
}
