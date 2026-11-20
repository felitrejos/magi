package main

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
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
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return nil, err
	}
	ws, _, err := sourceWorkspace(local, local.Name(), spec.Dir, host.Secrets, host.Exclude)
	if err != nil {
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
	id := store.NewTaskID()
	label, err := delegatorLabel(spec.Caller, ws.Name)
	if err != nil {
		return nil, err
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
		Label:    cmp.Or(label, fmt.Sprintf("%s · %s %s", ws.Name, spec.Kind, id)),
		Turn:     1,
		Keep:     spec.Keep,
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
		cleanup, cancel := context.WithTimeout(context.Background(), cleanupBudget)
		defer cancel()
		if _, rmErr := sh.Run(remote.With(cleanup), taskScript, "remove", t.Dir); rmErr != nil {
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
		if _, err := sh.RunInput(remote, `cat > "$1"`, []byte(spec.Prompt), t.Dir+"/prompt"); err != nil {
			return nil, removeCopy(err)
		}
		t.Session, t.Claude = store.NewSessionID(), spec.Claude
		command = t.delegateCommand(host, setup, false)
	case kindSession:
		claude := []string{expandHome(host.Claude, remoteHome), "--remote-control", ws.Name + " task " + id}
		if spec.Prompt != "" {
			claude = append(claude, "--", spec.Prompt)
		}
		command = append([]string{"sh", "-c", taskScript, "magi", "session", t.Dir}, claude...)
	}

	launcher := sh.Tmux{Machine: remote}
	tab, err := launcher.Start(group, t.Label, t.RunDir, command, spec.Kind == kindSession)
	if err != nil {
		return nil, removeCopy(err)
	}
	tab.SessionCreated = false
	if err := waitForTask(remote, t.Dir); err != nil {
		return nil, removeCopy(errors.Join(err, launcher.Discard(tab)))
	}
	t.Tab, t.Where, t.Starting = tab, launcher.Where(group), false
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
		cleanup, cancel := context.WithTimeout(context.Background(), cleanupBudget)
		defer cancel()
		launcher.Machine = remote.With(cleanup)
		_, cancelErr := sh.Run(launcher.Machine, taskScript, "cancel", t.Dir)
		return nil, removeCopy(errors.Join(err, cancelErr, launcher.Discard(tab)))
	}
	return res, nil
}

const launchWait = 20 * time.Second

func waitForTask(m sh.Machine, dir string) error {
	deadline := time.Now().Add(launchWait)
	for {
		if _, err := sh.Run(m, `[ -s "$1/pid" ] || [ -f "$1/exit" ]`, dir); err == nil {
			return nil
		} else if sh.ExitCode(err) != 1 {
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

func checkRoom(remote *sh.Remote, host Host, t *Task, action string) error {
	b, _ := host.budget()
	if reason := b.noRoom(time.Now(), nil, false); reason != "" {
		return &noRoomError{Host: t.Host, Reason: reason}
	}
	_, err := sh.Run(remote, taskScript, action, t.Dir, t.Kind, strconv.Itoa(b.MaxAgents), fmt.Sprintf("%.0f", b.MinFree))
	var ce *sh.Error
	if sh.ExitCode(err) == sh.ExitNoRoom && errors.As(err, &ce) {
		return &noRoomError{Host: t.Host, Reason: ce.Stderr}
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

func copyProject(local *sh.Local, remote *sh.Remote, ws Workspace, host Host, clone, checkout string) error {
	if ws.Git == nil {
		if _, err := sh.Run(remote, `mkdir -p -- "$1"`, checkout); err != nil {
			return err
		}
		return sh.Transfer(local, ws.Root, sh.Archive{Paths: []string{"."}, Exclude: host.Exclude}, remote, checkout)
	}
	if clone == "" {
		if err := initRepo(remote, checkout, ws.Git.Origin); err != nil {
			return err
		}
	} else if _, err := sh.Run(remote, `mkdir -p -- "$(dirname -- "$2")" && git clone --quiet --no-checkout -- "$1" "$2" && cd -- "$2" && git remote remove origin && if [ -n "$3" ]; then git remote add origin "$3"; fi`, clone, checkout, ws.Git.Origin); err != nil {
		return err
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
	_, err := sh.RunInput(remote, `cd -- "$1" && f=$(git rev-parse --path-format=absolute --git-path info/exclude) && mkdir -p -- "$(dirname -- "$f")" && cat >> "$f"`, []byte(strings.Join(lines, "\n")+"\n"), checkout)
	return err
}
