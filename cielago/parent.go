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

func (p *parentSession) wrap(command []string) []string {
	return append([]string{"env", parentEnv + "=" + p.ID}, command...)
}

func machineEmoji() string {
	if runtime.GOOS != "darwin" {
		return "🗄️"
	}
	model, err := exec.Command("sysctl", "-n", "hw.model").Output()
	if err == nil && strings.Contains(string(model), "MacBook") {
		return "💻"
	}
	return "🖥️"
}
