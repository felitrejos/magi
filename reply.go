package main

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"strings"
	"time"
)

const keepCopies = 24 * time.Hour

func newSessionID() string {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

func (t *Task) remoteHome() string {
	return strings.TrimSuffix(t.Dir, "/.cielago/tasks/"+t.ID)
}

func (t *Task) delegateCommand(host Host, setup string, resume bool) []string {
	claude := []string{expandHome(host.Claude, t.remoteHome())}
	if t.RunDir != t.Checkout {
		claude = append(claude, "--add-dir", t.Checkout)
	}
	if resume {
		claude = append(claude, "--resume", t.Session)
	} else {
		claude = append(claude, "--session-id", t.Session)
	}
	claude = append(claude, t.Claude...)
	return append([]string{"sh", "-c", taskScript, "cielago", "delegate", t.Dir, setup}, claude...)
}

func (t *Task) removeCopy(r *Remote) error {
	slug, err := projectSlug(t.RunDir)
	if err != nil {
		return err
	}
	if _, err := sh(r, taskScript, "remove", t.Dir, slug); err != nil {
		return fmt.Errorf("removing its copy at %s on %s failed: %w", t.Dir, t.Host, err)
	}
	return nil
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
	if t.Session == "" {
		return nil, fmt.Errorf("task %s was started by an older cielago that didn't keep its Claude session; start a new delegate", id)
	}
	if t.Outcome == nil {
		r, err := collectTask(id, false)
		if err != nil {
			return nil, err
		}
		if r.State.State == "running" || r.State.State == "starting" {
			return nil, fmt.Errorf("task %s is still working on its last turn; wait for it to finish", id)
		}
		t = r.Task
	}
	if !t.Outcome.Kept {
		return nil, fmt.Errorf("task %s's copy on %s is gone, so it can't continue; start a new delegate", id, t.Host)
	}
	unlock, err := lockTask(id)
	if err != nil {
		return nil, err
	}
	defer unlock()
	ctx, cancel := context.WithTimeout(ctx, time.Minute)
	defer cancel()
	r, host, err := t.remote(ctx)
	if err != nil {
		return nil, err
	}
	if _, err := shIn(r, `cd -- "$1" && rm -f exit status pid log view && cat > prompt`, []byte(message), t.Dir); err != nil {
		return nil, err
	}
	launcher := newLauncher(host, r)
	tab, err := launcher.Start(t.Group, t.tabLabel(), t.RunDir, t.delegateCommand(host, "", true), false)
	if err != nil {
		return nil, errors.Join(err, launcher.Discard(tab))
	}
	tab.SessionCreated = false
	if err := waitForTask(r, t.Dir); err != nil {
		return nil, errors.Join(err, launcher.Discard(tab))
	}
	t.History = append(t.History, *t.Outcome)
	t.Outcome, t.Tab, t.Reply = nil, tab, message
	t.Turn++
	if err := t.save(); err != nil {
		return nil, err
	}
	return &started{Task: t}, nil
}
