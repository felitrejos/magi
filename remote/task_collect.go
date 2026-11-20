package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"slices"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

const (
	keepCopies    = 24 * time.Hour
	keepCollected = 7 * 24 * time.Hour
)

func probeTask(r *sh.Remote, t *Task) (taskState, error) {
	out, err := sh.Run(r, taskScript, "status", t.Dir, fmt.Sprint(tailBytes))
	if err != nil {
		return taskState{}, err
	}
	state, rest, _ := strings.Cut(string(out), "\n")
	exit, tail, _ := strings.Cut(rest, "\n")
	return taskState{State: state, Exit: exit, Tail: tail}, nil
}

type taskReport struct {
	Task  *Task
	State taskState
}

func taskStatus(id string) (*taskReport, error) {
	t, err := loadTask(id)
	if err != nil {
		return nil, err
	}
	if t.Outcome != nil {
		return &taskReport{Task: t, State: taskState{State: "collected", Exit: t.Outcome.Exit, Tail: t.Outcome.Tail}}, nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	r, _, err := t.remote(ctx)
	if err != nil {
		return nil, err
	}
	st, err := probeTask(r, t)
	if err != nil {
		return nil, err
	}
	return &taskReport{Task: t, State: st}, nil
}

const (
	waitEvery    = 5 * time.Second
	waitFailures = 5
)

func waitTask(id string, every time.Duration) (*taskReport, error) {
	if _, err := loadTask(id); err != nil {
		return nil, err
	}
	for failures := 0; ; time.Sleep(every) {
		r, err := taskStatus(id)
		switch {
		case err != nil:
			if failures++; failures >= waitFailures {
				return nil, err
			}
		case r.State.State != "running" && r.State.State != "starting":
			return r, nil
		default:
			failures = 0
		}
	}
}

func collectTask(id string, cancelIt bool) (*taskReport, error) {
	if _, err := loadTask(id); err != nil {
		return nil, err
	}
	unlock, err := lockTask(id)
	if err != nil {
		return nil, err
	}
	defer unlock()
	t, err := loadTask(id)
	if err != nil {
		return nil, err
	}
	if t.Outcome != nil {
		if cancelIt && t.Outcome.Kept {
			if err := t.closeCopy(); err != nil {
				return nil, err
			}
		}
		return &taskReport{Task: t, State: taskState{State: "collected", Exit: t.Outcome.Exit, Tail: t.Outcome.Tail}}, nil
	}
	if t.Starting && time.Since(t.Created) < startBudget {
		return &taskReport{Task: t, State: taskState{State: "starting"}}, nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	r, host, err := t.remote(ctx)
	if err != nil {
		return nil, err
	}
	st, err := probeTask(r, t)
	if err != nil {
		return nil, err
	}
	cancelled := false
	if cancelIt && st.State == "running" {
		if _, err := sh.Run(r, taskScript, "cancel", t.Dir); err != nil {
			return nil, err
		}
		for i := 0; i < 40 && st.State == "running"; i++ {
			time.Sleep(250 * time.Millisecond)
			if st, err = probeTask(r, t); err != nil {
				return nil, err
			}
		}
		if st.State == "running" {
			return nil, fmt.Errorf("task %s on %s is still running after being told to stop; close its tab there (%s)", t.ID, t.Host, t.Label)
		}
		cancelled = true
	}
	if st.State == "running" {
		return &taskReport{Task: t, State: st}, nil
	}
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return nil, err
	}
	o := &Outcome{State: st.State, Exit: st.Exit, Ended: time.Now(), Tail: st.Tail}
	if cancelled {
		o.State, o.Exit = "cancelled", "cancelled"
	}
	o.Kept = t.Kind == kindDelegate && st.State == "done" && !cancelled
	if t.Kind == kindSession {
		o.Tail = ""
	} else if st.State != "gone" {
		if o.Log, err = fetchLog(r, t); err != nil {
			return nil, fmt.Errorf("copying task %s's log back from %s failed, so its copy there was kept; try again: %w", t.ID, t.Host, err)
		}
	}
	if t.Kind == kindDelegate && o.Log != "" {
		o.Message = finalMessage(o.Log)
	}
	if t.Kind != kindRun && st.State != "gone" && !cancelled {
		if err := bringChanges(local, r, host, t, o); err != nil {
			return nil, fmt.Errorf("%w; task %s's copy on %s was kept, so try again", err, t.ID, t.Host)
		}
	}
	var notes []error
	if err := (sh.Tmux{Machine: r}).Discard(t.Tab); err != nil && !sh.TabGone(err) {
		notes = append(notes, fmt.Errorf("closing its tab on %s failed: %w", t.Host, err))
	}
	if !o.Kept {
		if err := t.removeCopy(r); err != nil {
			notes = append(notes, err)
		}
	}
	if err := errors.Join(notes...); err != nil {
		o.Note = err.Error()
	}
	t.Outcome = o
	if err := t.save(); err != nil {
		return nil, err
	}
	return &taskReport{Task: t, State: taskState{State: "collected", Exit: o.Exit, Tail: o.Tail}}, nil
}

func fetchLog(r *sh.Remote, t *Task) (string, error) {
	name := t.ID + ".log"
	if t.Turn > 1 {
		name = fmt.Sprintf("%s-%d.log", t.ID, t.Turn)
	}
	path, err := taskPath(name)
	if err != nil {
		return "", err
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600)
	if err != nil {
		return "", err
	}
	cmd := r.Command(taskScript, "log", t.Dir)
	var stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = f, &stderr
	var runErr error
	if err := cmd.Run(); err != nil {
		runErr = &sh.Error{What: "sh", Stderr: strings.TrimSpace(stderr.String()), Err: err}
	}
	if err := errors.Join(runErr, f.Close()); err != nil {
		return "", err
	}
	return path, nil
}

func finalMessage(logPath string) string {
	data, err := os.ReadFile(logPath)
	if err != nil {
		return ""
	}
	lines := strings.Split(string(data), "\n")
	for _, line := range slices.Backward(lines) {
		var m struct {
			Type    string  `json:"type"`
			Subtype string  `json:"subtype"`
			Result  string  `json:"result"`
			IsError bool    `json:"is_error"`
			Turns   int     `json:"num_turns"`
			Cost    float64 `json:"total_cost_usd"`
		}
		if json.Unmarshal([]byte(line), &m) != nil || m.Type != "result" {
			continue
		}
		if m.IsError || m.Subtype != "success" {
			return fmt.Sprintf("Claude stopped with %s after %d turns ($%.2f). %s", m.Subtype, m.Turns, m.Cost, m.Result)
		}
		return fmt.Sprintf("%s\n\n(%d turns, $%.2f)", m.Result, m.Turns, m.Cost)
	}
	return ""
}

func bringChanges(local *sh.Local, r *sh.Remote, host Host, t *Task, o *Outcome) error {
	if !t.Git {
		files, err := taskPath(t.ID + ".files")
		if err != nil {
			return err
		}
		if err := os.MkdirAll(files, 0o700); err != nil {
			return err
		}
		if err := sh.Transfer(r, t.Checkout, sh.Archive{Paths: []string{"."}, Exclude: host.Exclude}, local, files); err != nil {
			return fmt.Errorf("copying the task's folder back failed: %w", err)
		}
		o.Files = files
		return nil
	}
	message := fmt.Sprintf("magi task %s on %s: %s", t.ID, t.Host, t.summary())
	if t.Turn > 1 {
		message = fmt.Sprintf("magi task %s, turn %d, on %s: %s", t.ID, t.Turn, t.Host, oneLine(t.Reply))
	}
	base := t.Base
	if t.Tip != "" {
		base = t.Tip
	}
	out, err := sh.Run(r, taskScript, "result", t.Dir, t.Checkout, t.Head, base, message)
	if err != nil {
		return fmt.Errorf("recording the task's changes failed: %w", err)
	}
	result := strings.TrimSpace(string(out))
	if result == "" {
		if t.Tip != "" {
			o.Branch = t.branch()
		}
		return nil
	}
	if !commitPattern.MatchString(result) {
		return fmt.Errorf("unexpected commit id %q from %s", result, t.Host)
	}
	if _, err := sh.Run(local, `dir=$1 ssh=$2 from=$3 ref=$4; cd -- "$dir" && GIT_SSH_COMMAND=$ssh git fetch --quiet --no-tags --no-write-fetch-head "$from" "+refs/magi/result:refs/heads/$ref"`,
		t.Project, r.SSHCommand(), r.Name()+":"+t.Checkout, t.branch()); err != nil {
		return fmt.Errorf("fetching the task's changes failed: %w", err)
	}
	o.Branch, o.From, t.Tip = t.branch(), base, result
	if stat, err := sh.Run(local, `cd -- "$1" && git diff --stat "$2" "$3" --`, t.Project, base, "refs/heads/"+t.branch()); err == nil {
		o.Diffstat = strings.TrimRight(string(stat), "\n")
	}
	if t.Kind == kindDelegate && !t.Keep && o.Diffstat != "" {
		applyChanges(local, t, o)
	}
	return nil
}

func applyChanges(local *sh.Local, t *Task, o *Outcome) {
	_, err := sh.Run(local, `cd -- "$1" && git diff --binary "$2" "$3" -- | git apply --binary`, t.Project, o.From, "refs/heads/"+o.Branch)
	if err != nil {
		o.Unapplied = strings.TrimPrefix(err.Error(), "on "+local.Name()+": ")
		return
	}
	o.Applied = true
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

func closeIdleCopy(id string) error {
	unlock, err := lockTask(id)
	if err != nil {
		return err
	}
	defer unlock()
	t, err := loadTask(id)
	if err != nil || t.Outcome == nil || !t.Outcome.Kept || time.Since(t.Outcome.Ended) < keepCopies {
		return err
	}
	return t.closeCopy()
}

func sweepTasks() string {
	tasks, err := listTasks()
	errs := []error{err}
	for _, t := range tasks {
		switch {
		case t.Outcome == nil:
		case t.Outcome.Kept && time.Since(t.Outcome.Ended) > keepCopies:
			if err := closeIdleCopy(t.ID); err != nil && !errors.Is(err, errTaskBusy) {
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
