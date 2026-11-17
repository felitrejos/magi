package main

import (
	"os/exec"
	"runtime"
	"strings"
)

const parentEnv = "CIELAGO_PARENT"

type parentSession struct {
	ID   string
	Name string
}

func delegatingSession(c caller, project string) (*parentSession, error) {
	if c.PID == 0 {
		return nil, nil
	}
	live, found, err := readLiveSession(c.PID)
	if halfWritten(err) {
		return nil, nil
	}
	if err != nil || !found || live.SessionID == "" {
		return nil, err
	}
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
