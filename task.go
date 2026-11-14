package main

import (
	"bytes"
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
	"strconv"
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
	Outcome  *Outcome  `json:"outcome,omitempty"`
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
}

type TaskSpec struct {
	Kind    string
	Host    string
	Dir     string
	Command string
	Prompt  string
	Setup   bool
	Claude  []string
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
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
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
	what = strings.Join(strings.Fields(what), " ")
	if r := []rune(what); len(r) > 70 {
		what = string(r[:67]) + "..."
	}
	if what == "" {
		what = "interactive session"
	}
	return what
}

func (t *Task) branch() string {
	return "cielago/task-" + t.ID
}

type started struct {
	Task    *Task
	Notes   []string
	ShownOn []string
}

func startTask(parent context.Context, spec TaskSpec) (*started, error) {
	ctx, cancel := context.WithTimeout(parent, startBudget)
	defer cancel()
	host, err := loadHost(spec.Host)
	if err != nil {
		return nil, err
	}
	local, err := newLocal(ctx)
	if err != nil {
		return nil, err
	}
	ws, err := readWorkspace(local, spec.Dir, host.Secrets, host.Exclude)
	if err != nil {
		return nil, err
	}
	home, err := homeDir(local)
	if err != nil {
		return nil, err
	}
	if err := refuseHome(ws, home, local.Name()); err != nil {
		return nil, err
	}
	remote, err := newRemote(ctx, host.SSH)
	if err != nil {
		return nil, err
	}
	remoteHome, err := homeDir(remote)
	if err != nil {
		return nil, err
	}
	group, err := localGroup(ws.MainRoot, host.LocalRoots)
	if err != nil {
		return nil, err
	}
	id := newTaskID()
	t := &Task{
		ID:       id,
		Kind:     spec.Kind,
		Host:     spec.Host,
		Created:  time.Now(),
		Project:  ws.Root,
		Cwd:      spec.Dir,
		Name:     ws.Name,
		Git:      ws.Git != nil,
		Subs:     len(ws.Submodules) > 0,
		Command:  spec.Command,
		Starting: true,
		Prompt:   spec.Prompt,
		Dir:      remoteHome + "/.cielago/tasks/" + id,
	}
	t.Checkout = t.Dir + "/checkout/" + ws.Name
	t.RunDir = filepath.Join(t.Checkout, ws.Subdir)
	if ws.Git != nil {
		t.Head, t.Base = ws.Git.Head, ws.Git.Worktree
	}
	res := &started{Task: t}

	var clone string
	if ws.Git != nil {
		clones, err := listClones(remote, host.Roots)
		if err != nil {
			return nil, err
		}
		for _, c := range clones {
			if sameRepository(c, ws) {
				clone = c.Dir
				group = serverGroup(clone, host.Roots, remoteHome)
				break
			}
		}
	}
	if err := t.save(); err != nil {
		return nil, err
	}
	removeCopy := func(err error) error {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		remote.SetContext(cleanup)
		if _, rmErr := sh(remote, taskScript, "remove", t.Dir); rmErr != nil {
			return fmt.Errorf("%w; removing the task's copy at %s on %s also failed: %v", err, t.Dir, spec.Host, rmErr)
		}
		return errors.Join(err, t.forget())
	}
	if err := reserve(remote, spec.Host, host, t); err != nil {
		return nil, errors.Join(err, t.forget())
	}
	if err := copyProject(local, remote, ws, host, clone, t.Checkout); err != nil {
		return nil, removeCopy(err)
	}

	setup := ""
	if spec.Setup && spec.Kind != kindSession {
		project, err := approvedProject(local, ws.Root)
		switch {
		case err != nil:
			res.Notes = append(res.Notes, err.Error())
		case project != nil && project.Setup != "":
			setup = project.Setup
			res.Notes = append(res.Notes, fmt.Sprintf("the project's setup (%s) runs first", setup))
		}
	}

	var command []string
	switch spec.Kind {
	case kindRun:
		command = []string{"sh", "-c", taskScript, "cielago", "run", t.Dir, setup, spec.Command}
	case kindDelegate:
		if _, err := shIn(remote, `cat > "$1"`, []byte(spec.Prompt), t.Dir+"/prompt"); err != nil {
			return nil, removeCopy(err)
		}
		claude := []string{expandHome(host.Claude, remoteHome)}
		if ws.Subdir != "." {
			claude = append(claude, "--add-dir", t.Checkout)
		}
		claude = append(claude, spec.Claude...)
		command = append([]string{"sh", "-c", taskScript, "cielago", "delegate", t.Dir, setup}, claude...)
	case kindSession:
		claude := []string{expandHome(host.Claude, remoteHome), "--remote-control", ws.Name + " task " + id}
		if spec.Prompt != "" {
			claude = append(claude, "--", spec.Prompt)
		}
		command = append([]string{"sh", "-c", taskScript, "cielago", "session", t.Dir}, claude...)
	default:
		return nil, removeCopy(fmt.Errorf("unknown task kind %q", spec.Kind))
	}

	launcher := newLauncher(host, remote)
	tab, err := launcher.Start(group, taskTabLabel(ws.Name, spec.Kind, id), t.RunDir, command, spec.Kind == kindSession)
	if err != nil {
		return nil, removeCopy(errors.Join(err, launcher.Discard(tab)))
	}
	tab.SessionCreated = false
	if err := waitForTask(remote, t.Dir); err != nil {
		return nil, removeCopy(errors.Join(err, launcher.Discard(tab)))
	}
	t.Tab, t.Where, t.Starting = tab, launcher.Where(group), false
	if spec.Kind == kindSession {
		if trusted, err := isTrusted(remote, t.RunDir); err == nil && !trusted {
			res.Notes = append(res.Notes, fmt.Sprintf("Claude there first asks whether to trust the copy's folder; the user answers Yes in that tab. To skip that for later sessions, the user can run claude once in %s/.cielago/tasks on %s and trust it", remoteHome, spec.Host))
		}
		var showErr error
		if res.ShownOn, showErr = launcher.Show(tab); showErr != nil {
			res.Notes = append(res.Notes, showErr.Error())
		}
	}
	if err := t.save(); err != nil {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		remote.SetContext(cleanup)
		sh(remote, taskScript, "cancel", t.Dir)
		return nil, removeCopy(errors.Join(err, launcher.Discard(tab)))
	}
	return res, nil
}

const launchWait = 20 * time.Second

func waitForTask(m Machine, dir string) error {
	deadline := time.Now().Add(launchWait)
	for {
		if _, err := sh(m, `[ -s "$1/pid" ] || [ -f "$1/exit" ]`, dir); err == nil {
			return nil
		} else if exitCode(err) != 1 {
			return err
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("the task's tab opened on %s, but its command didn't start within %s", m.Name(), launchWait)
		}
		select {
		case <-m.Context().Done():
			return m.Context().Err()
		case <-time.After(150 * time.Millisecond):
		}
	}
}

func taskTabLabel(name, kind, id string) string {
	return fmt.Sprintf("%s · %s %s", name, kind, id)
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

const exitNoRoom = 11

func reserve(remote *Remote, name string, host Host, t *Task) error {
	b, err := host.budget()
	if err != nil {
		return err
	}
	if reason := b.noRoom(time.Now(), nil, false); reason != "" {
		return &noRoomError{Host: name, Reason: reason}
	}
	_, err = sh(remote, taskScript, "reserve", t.Dir, t.Kind, strconv.Itoa(b.MaxAgents), fmt.Sprintf("%.0f", b.MinFree))
	var ce *commandError
	if exitCode(err) == exitNoRoom && errors.As(err, &ce) {
		return &noRoomError{Host: name, Reason: ce.stderr}
	}
	return err
}

func excludeLine(path string) string {
	var b strings.Builder
	b.WriteByte('/')
	for _, r := range path {
		if strings.ContainsRune(`\*?[`, r) {
			b.WriteByte('\\')
		}
		b.WriteRune(r)
	}
	return b.String()
}

func copyProject(local *Local, remote *Remote, ws Workspace, host Host, clone, checkout string) error {
	if ws.Git == nil {
		if _, err := sh(remote, `mkdir -p -- "$1"`, checkout); err != nil {
			return err
		}
		return transfer(local, ws.Root, Archive{Paths: []string{"."}, Exclude: host.Exclude}, remote, checkout)
	}
	if clone != "" {
		if _, err := sh(remote, `mkdir -p -- "$(dirname -- "$2")" && git clone --quiet --no-checkout -- "$1" "$2" && cd -- "$2" && git remote remove origin && if [ -n "$3" ]; then git remote add origin "$3"; fi`, clone, checkout, ws.Git.Origin); err != nil {
			return err
		}
	} else {
		if _, err := sh(remote, `mkdir -p -- "$(dirname -- "$1")" && git init --quiet "$1" && if [ -n "$2" ]; then git -C "$1" remote add origin "$2"; fi`, checkout, ws.Git.Origin); err != nil {
			return err
		}
		if ws.Git.Origin != "" {
			prefetchOrigin(remote, checkout)
		}
	}
	if err := sendGit(local, remote, ws, checkout, "copy"); err != nil {
		return err
	}
	if len(ws.Secrets) == 0 {
		return nil
	}
	lines := make([]string, len(ws.Secrets))
	for i, secret := range ws.Secrets {
		lines[i] = excludeLine(secret)
	}
	_, err := shIn(remote, `cd -- "$1" && f=$(git rev-parse --path-format=absolute --git-path info/exclude) && mkdir -p -- "$(dirname -- "$f")" && cat >> "$f"`, []byte(strings.Join(lines, "\n")+"\n"), checkout)
	return err
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

func probeTask(r *Remote, t *Task) (taskState, error) {
	out, err := sh(r, taskScript, "status", t.Dir, fmt.Sprint(tailBytes))
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
		return nil, fmt.Errorf("task %s is being collected by another cielago right now; try again in a moment", id)
	}
	return func() { f.Close() }, nil
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
		if _, err := sh(r, taskScript, "cancel", t.Dir); err != nil {
			return nil, err
		}
		for i := 0; i < 40 && st.State == "running"; i++ {
			time.Sleep(250 * time.Millisecond)
			if st, err = probeTask(r, t); err != nil {
				return nil, err
			}
		}
		if st.State == "running" {
			return nil, fmt.Errorf("task %s on %s is still running after being told to stop; close its tab there (%s)", t.ID, t.Host, t.tabLabel())
		}
		cancelled = true
	}
	if st.State == "running" {
		return &taskReport{Task: t, State: st}, nil
	}
	local, err := newLocal(ctx)
	if err != nil {
		return nil, err
	}
	o := &Outcome{State: st.State, Exit: st.Exit, Ended: time.Now(), Tail: st.Tail}
	if cancelled {
		o.State, o.Exit = "cancelled", "cancelled"
	}
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
	if err := newLauncher(host, r).Discard(t.Tab); err != nil && !tabGone(err) {
		notes = append(notes, fmt.Errorf("closing its tab on %s failed: %w", t.Host, err))
	}
	if _, err := sh(r, taskScript, "remove", t.Dir); err != nil {
		notes = append(notes, fmt.Errorf("removing its copy at %s on %s failed: %w", t.Dir, t.Host, err))
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

func tabGone(err error) bool {
	msg := err.Error()
	return strings.Contains(msg, "target") && strings.Contains(msg, "not found") || strings.Contains(msg, "can't find window")
}

func fetchLog(r *Remote, t *Task) (string, error) {
	dir, err := tasksDir()
	if err != nil {
		return "", err
	}
	path := filepath.Join(dir, t.ID+".log")
	f, err := os.OpenFile(path, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600)
	if err != nil {
		return "", err
	}
	cmd := r.Command(taskScript, "log", t.Dir)
	var stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = f, &stderr
	var runErr error
	if err := cmd.Run(); err != nil {
		runErr = &commandError{what: "sh", stderr: strings.TrimSpace(stderr.String()), err: err}
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

func bringChanges(local *Local, r *Remote, host Host, t *Task, o *Outcome) error {
	if !t.Git {
		dir, err := tasksDir()
		if err != nil {
			return err
		}
		files := filepath.Join(dir, t.ID+".files")
		if err := os.MkdirAll(files, 0o700); err != nil {
			return err
		}
		if err := transfer(r, t.Checkout, Archive{Paths: []string{"."}, Exclude: host.Exclude}, local, files); err != nil {
			return fmt.Errorf("copying the task's folder back failed: %w", err)
		}
		o.Files = files
		return nil
	}
	message := fmt.Sprintf("cielago task %s on %s: %s", t.ID, t.Host, t.summary())
	out, err := sh(r, taskScript, "result", t.Dir, t.Checkout, t.Head, t.Base, message)
	if err != nil {
		return fmt.Errorf("recording the task's changes failed: %w", err)
	}
	result := strings.TrimSpace(string(out))
	if result == "" {
		return nil
	}
	if !commitPattern.MatchString(result) {
		return fmt.Errorf("unexpected commit id %q from %s", result, t.Host)
	}
	if _, err := sh(local, `dir=$1 ssh=$2 from=$3 ref=$4; cd -- "$dir" && GIT_SSH_COMMAND=$ssh git fetch --quiet --no-tags --no-write-fetch-head "$from" "+refs/cielago/result:refs/heads/$ref"`,
		t.Project, r.sshCommand(), r.host+":"+t.Checkout, t.branch()); err != nil {
		return fmt.Errorf("fetching the task's changes failed: %w", err)
	}
	o.Branch = t.branch()
	if stat, err := sh(local, `cd -- "$1" && git diff --stat "$2" "$3" --`, t.Project, t.Base, "refs/heads/"+t.branch()); err == nil {
		o.Diffstat = strings.TrimRight(string(stat), "\n")
	}
	return nil
}
