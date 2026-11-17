package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"syscall"
	"time"
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
	Cwd      string    `json:"cwd"`
	Name     string    `json:"name"`
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
	Tab      Tab       `json:"tab"`
	Label    string    `json:"tab_label,omitempty"`
	Parent   string    `json:"parent_session,omitempty"`
	Outcome  *Outcome  `json:"outcome,omitempty"`
	Group    string    `json:"group,omitempty"`
	Session  string    `json:"claude_session,omitempty"`
	Claude   []string  `json:"claude_args,omitempty"`
	Turn     int       `json:"turn,omitempty"`
	Reply    string    `json:"reply,omitempty"`
	Tip      string    `json:"tip,omitempty"`
	History  []Outcome `json:"history,omitempty"`
	State    string    `json:"state"`
	Caller   int       `json:"caller_pid,omitempty"`
	CallerAt int64     `json:"caller_started,omitempty"`
}

type Outcome struct {
	State    string    `json:"state"`
	Exit     string    `json:"exit"`
	Ended    time.Time `json:"ended"`
	Tail     string    `json:"tail"`
	Log      string    `json:"log"`
	Message  string    `json:"message,omitempty"`
	Branch   string    `json:"branch,omitempty"`
	Diffstat string    `json:"diffstat,omitempty"`
	Files    string    `json:"files,omitempty"`
	Note     string    `json:"note,omitempty"`
	Kept     bool      `json:"kept,omitempty"`
	Closed   bool      `json:"closed,omitempty"`
}

type TaskSpec struct {
	Kind    string
	Host    string
	Dir     string
	Command string
	Prompt  string
	Setup   bool
	Claude  []string
	Caller  caller
}

func tasksDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".cielago", "tasks"), nil
}

func newTaskID() string {
	b := make([]byte, 4)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b)
}

func (t *Task) save() error {
	dir, err := tasksDir()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	data, err := json.MarshalIndent(t, "", "  ")
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".task-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	_, err = f.Write(data)
	if err := errors.Join(err, f.Close()); err != nil {
		return err
	}
	return os.Rename(f.Name(), filepath.Join(dir, t.ID+".json"))
}

func loadTask(id string) (*Task, error) {
	if !taskIDPattern.MatchString(id) {
		return nil, fmt.Errorf("%q isn't a task id; task ids are 8 hex characters", id)
	}
	dir, err := tasksDir()
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(filepath.Join(dir, id+".json"))
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
	dir, err := tasksDir()
	if err != nil {
		return nil, err
	}
	names, err := filepath.Glob(filepath.Join(dir, "*.json"))
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
	return "cielago/task-" + t.ID
}

func (t *Task) forget() error {
	dir, err := tasksDir()
	if err != nil {
		return err
	}
	var errs []error
	for _, path := range []string{t.ID + ".json", t.ID + ".log", t.ID + ".lock", t.ID + ".files"} {
		if err := os.RemoveAll(filepath.Join(dir, path)); err != nil {
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

func (t *Task) remote(ctx context.Context) (*Remote, Host, error) {
	host, err := loadHost(t.Host)
	if err != nil {
		return nil, host, err
	}
	r, err := newRemote(ctx, host.SSH)
	return r, host, err
}

func (t *Task) noteState(state string) error {
	if t.State == state {
		return nil
	}
	unlock, err := lockTask(t.ID)
	if errors.Is(err, errTaskBusy) {
		return nil
	}
	if err != nil {
		return err
	}
	defer unlock()
	fresh, err := loadTask(t.ID)
	if err != nil {
		return err
	}
	if fresh.Outcome != nil || fresh.Starting || fresh.Turn != t.Turn {
		return nil
	}
	fresh.State = state
	return fresh.save()
}

var errTaskBusy = errors.New("being collected by another cielago right now; try again in a moment")

func lockTask(id string) (func(), error) {
	dir, err := tasksDir()
	if err != nil {
		return nil, err
	}
	f, err := os.OpenFile(filepath.Join(dir, id+".lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		f.Close()
		return nil, fmt.Errorf("task %s is %w", id, errTaskBusy)
	}
	return func() { f.Close() }, nil
}
