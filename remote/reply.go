package main

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func (t *Task) delegateCommand(program, setup string, resume bool) []string {
	claude := []string{"env", delegateVar + "=1", program}
	if t.RunDir != t.Checkout {
		claude = append(claude, "--add-dir", t.Checkout)
	}
	if resume {
		claude = append(claude, "--resume", t.Session)
	} else {
		claude = append(claude, "--session-id", t.Session)
	}
	claude = append(claude, t.Claude...)
	return append([]string{"sh", "-c", taskScript, "magi", "delegate", t.Dir, setup}, claude...)
}

func replyTask(ctx context.Context, id, message string) (*started, error) {
	if strings.TrimSpace(message) == "" {
		return nil, errors.New("message is empty")
	}
	t, err := loadTask(id)
	if err != nil {
		return nil, err
	}
	if t.Kind != kindDelegate {
		return nil, fmt.Errorf("task %s is a %s task; only a delegated Claude takes follow-ups", id, t.Kind)
	}
	if t.Outcome == nil {
		r, err := collectTask(id, false)
		if err != nil {
			return nil, err
		}
		if r.State.State == "running" || r.State.State == "starting" {
			return nil, fmt.Errorf("task %s is still working on its last turn; wait for it to finish", id)
		}
	}
	unlock, err := lockTask(id)
	if err != nil {
		return nil, err
	}
	defer unlock()
	if t, err = loadTask(id); err != nil {
		return nil, err
	}
	if t.Outcome == nil {
		return nil, fmt.Errorf("task %s is already working on another follow-up; wait for it to finish", id)
	}
	if !t.Outcome.Kept {
		return nil, fmt.Errorf("task %s's copy on %s is gone, so it can't continue; start a new delegate", id, t.Host)
	}
	ctx, cancel := context.WithTimeout(ctx, time.Minute)
	defer cancel()
	r, host, err := t.remote(ctx)
	if err != nil {
		return nil, err
	}
	if err := checkRoom(r, host, t, "room"); err != nil {
		return nil, err
	}
	if _, err := sh.RunInput(r, `cd -- "$1" && rm -f exit status pid log view && cat > prompt`, []byte(message), t.Dir); err != nil {
		return nil, err
	}
	launcher := sh.Tmux{Machine: r}
	tab, err := launcher.Start(t.Group, t.Label, t.RunDir, t.delegateCommand(r.Claude(), "", true), false)
	if err != nil {
		return nil, err
	}
	tab.SessionCreated = false
	if err := waitForTask(r, t.Dir); err != nil {
		return nil, errors.Join(err, launcher.Discard(tab))
	}
	t.Outcome, t.Tab, t.Reply = nil, tab, message
	t.Turn++
	if err := t.save(); err != nil {
		return nil, err
	}
	return &started{Task: t}, nil
}
