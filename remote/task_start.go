package main

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

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
	remote, err := newRemote(ctx, host)
	if err != nil {
		return nil, err
	}
	remoteHome, err := homeDir(remote)
	if err != nil {
		return nil, err
	}
	group := localGroup(ws.MainRoot, host.LocalRoots)
	id := newTaskID()
	label, parentID := taskTabLabel(ws.Name, spec.Kind, id), ""
	delegator, err := delegatingSession(spec.Caller, ws.Name)
	if err != nil {
		return nil, err
	}
	if delegator != nil {
		label, parentID = delegator.tabLabel(), delegator.ID
	}
	t := &Task{
		ID:       id,
		Kind:     spec.Kind,
		Host:     spec.Host,
		Created:  time.Now(),
		Project:  ws.Root,
		Name:     ws.Name,
		Git:      ws.Git != nil,
		Subs:     len(ws.Submodules) > 0,
		Command:  spec.Command,
		Starting: true,
		Prompt:   spec.Prompt,
		Dir:      remoteHome + "/.magi/tasks/" + id,
		Label:    label,
		Parent:   parentID,
		Turn:     1,
		Keep:     spec.Keep,
		State:    "starting",
		Caller:   spec.Caller.PID,
		CallerAt: spec.Caller.Started,
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
	t.Group = group
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
	if err := checkRoom(remote, host, t, "reserve"); err != nil {
		return nil, errors.Join(err, t.forget())
	}
	if err := copyProject(local, remote, ws, host, clone, t.Checkout); err != nil {
		return nil, removeCopy(err)
	}

	setup := ""
	if spec.Setup {
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
		command = []string{"sh", "-c", taskScript, "magi", "run", t.Dir, setup, spec.Command}
	case kindDelegate:
		if _, err := shIn(remote, `cat > "$1"`, []byte(spec.Prompt), t.Dir+"/prompt"); err != nil {
			return nil, removeCopy(err)
		}
		t.Session, t.Claude = newSessionID(), spec.Claude
		command = t.delegateCommand(host, setup, false)
	case kindSession:
		claude := []string{expandHome(host.Claude, remoteHome), "--remote-control", ws.Name + " task " + id}
		if spec.Prompt != "" {
			claude = append(claude, "--", spec.Prompt)
		}
		command = append([]string{"sh", "-c", taskScript, "magi", "session", t.Dir}, claude...)
	}

	launcher := Tmux{remote: remote}
	tab, err := launcher.Start(group, t.Label, t.RunDir, withParent(t.Parent, command), spec.Kind == kindSession)
	if err != nil {
		return nil, removeCopy(errors.Join(err, launcher.Discard(tab)))
	}
	tab.SessionCreated = false
	if err := waitForTask(remote, t.Dir); err != nil {
		return nil, removeCopy(errors.Join(err, launcher.Discard(tab)))
	}
	t.Tab, t.Where, t.Starting, t.State = tab, launcher.Where(group), false, "running"
	if spec.Kind == kindSession {
		if trusted, err := isTrusted(remote, t.RunDir); err == nil && !trusted {
			res.Notes = append(res.Notes, fmt.Sprintf("Claude there first asks whether to trust the copy's folder; the user answers Yes in that tab. To skip that for later sessions, the user can run claude once in %s/.magi/tasks on %s and trust it", remoteHome, spec.Host))
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
		_, cancelErr := sh(remote, taskScript, "cancel", t.Dir)
		return nil, removeCopy(errors.Join(err, cancelErr, launcher.Discard(tab)))
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

const exitNoRoom = 11

func checkRoom(remote *Remote, host Host, t *Task, action string) error {
	b, _ := host.budget()
	if reason := b.noRoom(time.Now(), nil, false); reason != "" {
		return &noRoomError{Host: t.Host, Reason: reason}
	}
	_, err := sh(remote, taskScript, action, t.Dir, t.Kind, strconv.Itoa(b.MaxAgents), fmt.Sprintf("%.0f", b.MinFree))
	var ce *commandError
	if exitCode(err) == exitNoRoom && errors.As(err, &ce) {
		return &noRoomError{Host: t.Host, Reason: ce.stderr}
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
