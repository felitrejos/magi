package main

import (
	"context"
	"os/exec"
	"runtime"
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
	return machineEmoji() + " ← " + p.Name
}

func withParent(id string, command []string) []string {
	if id == "" {
		return command
	}
	return append([]string{"env", parentEnv + "=" + id}, command...)
}

func machineEmoji() string {
	if runtime.GOOS != "darwin" {
		return "🗄️"
	}
	battery, err := exec.Command("pmset", "-g", "batt").Output()
	if err == nil && strings.Contains(string(battery), "InternalBattery") {
		return "💻"
	}
	return "🖥️"
}
