package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const (
	kindRun      = "run"
	kindDelegate = "delegate"
	kindSession  = "session"

	tailBytes   = 6000
	startBudget = 5 * time.Minute
)

var taskIDPattern = regexp.MustCompile(`^[0-9a-f]{8}$`)

type Task struct {
	ID       string    `json:"id"`
	Kind     string    `json:"kind"`
	Host     string    `json:"host"`
	Created  time.Time `json:"created"`
	Project  string    `json:"project"`
	Git      bool      `json:"git"`
	Subs     bool      `json:"submodules,omitempty"`
	Starting bool      `json:"starting,omitempty"`
	Head     string    `json:"head,omitempty"`
	Base     string    `json:"base,omitempty"`
	Command  string    `json:"command,omitempty"`
	Prompt   string    `json:"prompt,omitempty"`
	Dir      string    `json:"dir"`
	Checkout string    `json:"checkout"`
	RunDir   string    `json:"run_dir"`
	Where    string    `json:"where"`
	Tab      sh.Tab    `json:"tab"`
	Label    string    `json:"tab_label,omitempty"`
	Outcome  *Outcome  `json:"outcome,omitempty"`
	Group    string    `json:"group,omitempty"`
	Session  string    `json:"claude_session,omitempty"`
	Claude   []string  `json:"claude_args,omitempty"`
	Turn     int       `json:"turn,omitempty"`
	Reply    string    `json:"reply,omitempty"`
	Tip      string    `json:"tip,omitempty"`
	Keep     bool      `json:"keep_branch,omitempty"`
}

type Outcome struct {
	State     string    `json:"state"`
	Exit      string    `json:"exit"`
	Ended     time.Time `json:"ended"`
	Tail      string    `json:"tail"`
	Log       string    `json:"log"`
	Message   string    `json:"message,omitempty"`
	Branch    string    `json:"branch,omitempty"`
	Diffstat  string    `json:"diffstat,omitempty"`
	From      string    `json:"from,omitempty"`
	Applied   bool      `json:"applied,omitempty"`
	Unapplied string    `json:"unapplied,omitempty"`
	Files     string    `json:"files,omitempty"`
	Note      string    `json:"note,omitempty"`
	Kept      bool      `json:"kept,omitempty"`
	Closed    bool      `json:"closed,omitempty"`
}

type TaskSpec struct {
	Kind    string
	Host    string
	Dir     string
	Command string
	Prompt  string
	Setup   bool
	Claude  []string
	Keep    bool
	Caller  int
}

func taskPath(name string) (string, error) {
	return store.Dir(filepath.Join("tasks", name))
}

func (t *Task) save() error {
	path, err := taskPath(t.ID + ".json")
	if err != nil {
		return err
	}
	data, err := json.MarshalIndent(t, "", "  ")
	if err != nil {
		return err
	}
	return store.WriteAtomic(path, data)
}

func loadTask(id string) (*Task, error) {
	if !taskIDPattern.MatchString(id) {
		return nil, fmt.Errorf("%q isn't a task id; task ids are 8 hex characters", id)
	}
	path, err := taskPath(id + ".json")
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, fmt.Errorf("no task %s on this machine", id)
	}
	if err != nil {
		return nil, err
	}
	var t Task
	if err := json.Unmarshal(data, &t); err != nil {
		return nil, fmt.Errorf("task %s: %w", id, err)
	}
	return &t, nil
}

func listTasks() ([]*Task, error) {
	pattern, err := taskPath("*.json")
	if err != nil {
		return nil, err
	}
	names, err := filepath.Glob(pattern)
	if err != nil {
		return nil, err
	}
	var tasks []*Task
	for _, name := range names {
		t, err := loadTask(strings.TrimSuffix(filepath.Base(name), ".json"))
		if err == nil {
			tasks = append(tasks, t)
		}
	}
	slices.SortFunc(tasks, func(a, b *Task) int { return b.Created.Compare(a.Created) })
	return tasks, nil
}

func (t *Task) summary() string {
	what := t.Command
	if t.Kind != kindRun {
		what = t.Prompt
	}
	if what = oneLine(what); what == "" {
		what = "interactive session"
	}
	return what
}

func oneLine(text string) string {
	text = strings.Join(strings.Fields(text), " ")
	if r := []rune(text); len(r) > 70 {
		text = string(r[:67]) + "..."
	}
	return text
}

func (t *Task) branch() string {
	return "magi/task-" + t.ID
}

func (t *Task) forget() error {
	base, err := taskPath(t.ID)
	if err != nil {
		return err
	}
	turnLogs, err := filepath.Glob(base + "-*.log")
	if err != nil {
		return err
	}
	var errs []error
	for _, path := range append([]string{base + ".json", base + ".log", base + ".lock", base + ".files"}, turnLogs...) {
		if err := os.RemoveAll(path); err != nil {
			errs = append(errs, err)
		}
	}
	return errors.Join(errs...)
}

type taskState struct {
	State string
	Exit  string
	Tail  string
}

func (t *Task) remote(ctx context.Context) (*sh.Remote, Host, error) {
	host, err := loadHost(t.Host)
	if err != nil {
		return nil, host, err
	}
	r, err := newRemote(ctx, host)
	return r, host, err
}

var errTaskBusy = errors.New("being collected by another magi right now; try again in a moment")

func lockTask(id string) (func(), error) {
	path, err := taskPath(id + ".lock")
	if err != nil {
		return nil, err
	}
	return store.Lock(path, fmt.Errorf("task %s is %w", id, errTaskBusy))
}
