package main

import (
	"cmp"
	"context"
	"os"
	"slices"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func delegatorLabel(pid int, project string) (string, error) {
	if pid == 0 {
		return "", nil
	}
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return "", err
	}
	sessions, err := liveSessions(local)
	if err != nil {
		return "", err
	}
	i := slices.IndexFunc(sessions, func(s liveSession) bool { return s.PID == pid })
	if i < 0 {
		return "", nil
	}
	return machineLabel() + " ← " + cmp.Or(strings.TrimSpace(sessions[i].Name), project), nil
}

func machineLabel() string {
	name, _ := os.Hostname()
	short, _, _ := strings.Cut(name, ".")
	return strings.ToLower(short)
}
