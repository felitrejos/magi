package main

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

const (
	keepCopies    = 24 * time.Hour
	keepCollected = 7 * 24 * time.Hour
)

func (t *Task) remoteHome() string {
	return strings.TrimSuffix(t.Dir, "/.magi/tasks/"+t.ID)
}

func (t *Task) delegateCommand(host Host, setup string, resume bool) []string {
	claude := []string{"env", delegateVar + "=1", expandHome(host.Claude, t.remoteHome())}
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

func (t *Task) removeCopy(r *sh.Remote) error {
	slug := ""
	if t.Kind == kindDelegate {
		var err error
		if slug, err = projectSlug(t.RunDir); err != nil {
			return err
		}
	}
	if _, err := sh.Run(r, taskScript, "remove", t.Dir, slug); err != nil {
		return fmt.Errorf("removing its copy at %s on %s failed: %w", t.Dir, t.Host, err)
	}
	return nil
}

func closeIdleCopy(id string) (bool, error) {
	unlock, err := lockTask(id)
	if err != nil {
		return false, err
	}
	defer unlock()
	t, err := loadTask(id)
	if err != nil || t.Outcome == nil || !t.Outcome.Kept || time.Since(t.Outcome.Ended) < keepCopies {
		return false, err
	}
	return true, t.closeCopy()
}

func sweepTasks() string {
	tasks, err := listTasks()
	errs := []error{err}
	for _, t := range tasks {
		switch {
		case t.Outcome == nil:
		case t.Outcome.Kept && time.Since(t.Outcome.Ended) > keepCopies:
			if _, err := closeIdleCopy(t.ID); err != nil && !errors.Is(err, errTaskBusy) {
				errs = append(errs, fmt.Errorf("task %s: %w", t.ID, err))
			}
		case !t.Outcome.Kept && time.Since(t.Outcome.Ended) > keepCollected:
			if err := t.forget(); err != nil {
				errs = append(errs, fmt.Errorf("task %s: %w", t.ID, err))
			}
		}
	}
	if err := errors.Join(errs...); err != nil {
		return fmt.Sprintf("Note: tidying up old tasks failed: %v\n", err)
	}
	return ""
}

func (t *Task) closeCopy() error {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	r, _, err := t.remote(ctx)
	if err != nil {
		return err
	}
	if err := t.removeCopy(r); err != nil {
		return err
	}
	t.Outcome.Kept, t.Outcome.Closed = false, true
	return t.save()
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
	tab, err := launcher.Start(t.Group, t.Label, t.RunDir, t.delegateCommand(host, "", true), false)
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
