package main

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const (
	prepareBudget = 80 * time.Second
	launchBudget  = 45 * time.Second
)

type Handoff struct {
	HostName       string
	Host           Host
	SessionID      string
	TranscriptPath string
	Cwd            string
	Note           string
	Progress       func(step string)
	Codex          bool
	Rollout        string
	Retire         func() error
}

type Step struct {
	Name string
	Took time.Duration
}

type Result struct {
	Steps     []Step
	ShownOn   []string
	Where     string
	Notes     []error
	Ports     []int
	NewFolder string
	From      string
	FromDir   string
	ToDir     string
	Stray     *Stray
}

type Stray struct {
	Host  string `json:"host"`
	Tab   sh.Tab `json:"tab"`
	Codex bool   `json:"codex"`
}

func tabLabel(name string, codex bool) string {
	if codex {
		return name + " · codex"
	}
	return name + " · claude"
}

func (r *Result) note(err error) {
	if err != nil {
		r.Notes = append(r.Notes, err)
	}
}

func (r Result) Noted() string {
	notes := make([]string, len(r.Notes))
	for i, note := range r.Notes {
		notes[i] = strings.ReplaceAll(note.Error(), "\n", "; ")
	}
	return strings.Join(notes, "; ")
}

func (r Result) Forwarded() string {
	if len(r.Ports) == 0 {
		return ""
	}
	ports := make([]string, len(r.Ports))
	for i, port := range r.Ports {
		ports[i] = fmt.Sprintf("localhost:%d", port)
	}
	return fmt.Sprintf("%s here now reaches it there", strings.Join(ports, ", "))
}

func (r Result) Total() time.Duration {
	var total time.Duration
	for _, s := range r.Steps {
		total += s.Took
	}
	return total
}

func (r Result) Summary() string {
	parts := make([]string, len(r.Steps))
	for i, s := range r.Steps {
		parts[i] = fmt.Sprintf("%s %.1fs", s.Name, s.Took.Seconds())
	}
	return fmt.Sprintf("%.1fs (%s)", r.Total().Seconds(), strings.Join(parts, ", "))
}

type stepper struct {
	steps    []Step
	progress func(step string)
}

func (s *stepper) run(name string, fn func() error) error {
	if s.progress != nil {
		s.progress(name)
	}
	start := time.Now()
	err := fn()
	s.steps = append(s.steps, Step{name, time.Since(start)})
	if err != nil {
		return fmt.Errorf("%s: %w", name, err)
	}
	return nil
}

type handoffRun struct {
	Handoff
	parent      context.Context
	st          stepper
	res         Result
	local       *sh.Local
	remote      *sh.Remote
	ws          Workspace
	where       Placement
	remoteDir   string
	project     *Project
	launcher    sh.Tmux
	tab         sh.Tab
	synced      bool
	launchedAt  time.Time
	setupStatus string
	rollout     string
	written     int
}

func (h Handoff) Run(parent context.Context) (Result, error) {
	ctx, cancel := context.WithTimeout(parent, prepareBudget)
	defer cancel()
	r := &handoffRun{Handoff: h, parent: parent, st: stepper{progress: h.Progress}}
	err := r.run(ctx)
	r.res.Steps = r.st.steps
	return r.res, err
}

func (r *handoffRun) run(ctx context.Context) error {
	var err error
	if r.local, err = sh.NewLocal(ctx); err != nil {
		return err
	}
	if err := r.st.run("inspect", func() error { return r.inspect(ctx) }); err != nil {
		return err
	}
	project, err := approvedProject(r.local, r.ws.Root)
	r.project = project
	r.res.note(err)
	r.remoteDir = filepath.Join(r.where.Root, r.ws.Subdir)
	r.res.NewFolder, r.res.From, r.res.FromDir, r.res.ToDir = r.where.NewFolder, r.local.Name(), r.Cwd, r.remoteDir
	r.launcher = sh.Tmux{Machine: r.remote}

	if err := r.st.run("sync", func() error {
		return syncProject(r.local, r.remote, r.ws, r.where, r.Host.Exclude, func() error { return pushGit(r.local, r.remote, r.ws, r.where.Root) })
	}); err != nil {
		return r.undo(err, false)
	}
	r.synced = true
	var sidecar string
	if err := r.st.run("transcript", func() (err error) {
		if r.Codex {
			r.rollout, r.written, err = moveRollout(r.local, r.TranscriptPath, r.Rollout, r.ws.Root, r.remote, r.where.Root)
			return err
		}
		sidecar, err = moveTranscript(r.local, r.TranscriptPath, r.ws, r.remote, r.where, r.remoteDir)
		return err
	}); err != nil {
		return r.undo(err, false)
	}
	if err := r.st.run("launch", func() error { return r.launch(sidecar) }); err != nil {
		return r.undo(err, false)
	}
	start, cancelStart := context.WithTimeout(r.parent, launchBudget)
	defer cancelStart()
	remote := r.remote.With(start)
	if err := r.st.run("start", func() error {
		if r.Codex {
			return waitForCodex(remote, r.rollout, r.written)
		}
		return waitForRemoteControl(remote, r.SessionID, r.launchedAt)
	}); err != nil {
		return r.undo(err, true)
	}
	r.finish()
	if err := r.parent.Err(); err != nil {
		return r.rollback(fmt.Errorf("the handoff was interrupted: %w", err))
	}
	if r.Retire != nil {
		if err := r.Retire(); err != nil {
			return r.rollback(fmt.Errorf("closing the session here failed, so the copy on %s was stopped: %w", r.HostName, err))
		}
	}
	return nil
}

func (r *handoffRun) rollback(err error) error {
	err = r.undo(err, false)
	cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if r.ws.Git != nil {
		if unparkErr := unparkRepo(r.local.With(cleanup), r.ws.Root); unparkErr != nil {
			err = fmt.Errorf("%w; unparking this checkout also failed: %v", err, unparkErr)
		}
	}
	return err
}

func (r *handoffRun) inspect(ctx context.Context) error {
	var err error
	if r.ws, err = readWorkspace(r.local, r.Cwd, r.Host.Secrets, r.Host.Exclude); err != nil {
		return err
	}
	home, err := homeDir(r.local)
	if err != nil {
		return err
	}
	if err := refuseHome(r.ws, home, r.local.Name()); err != nil {
		return err
	}
	if r.remote, err = newRemote(ctx, r.Host); err != nil {
		return err
	}
	if r.where, err = place(r.remote, r.ws, r.Host.Roots, localGroup(r.ws.MainRoot, r.Host.LocalRoots), r.Host.Exclude); err != nil {
		return err
	}
	if err := refuseUntested(r.Codex, r.local, r.remote, expandHome(r.Host.program(r.Codex), r.where.Home)); err != nil {
		return err
	}
	if r.Codex {
		return nil
	}
	running, err := runningThere(r.remote, r.SessionID)
	if err != nil {
		return err
	}
	if running {
		return fmt.Errorf("session %s is already running on %s; close it there before handing it off again", store.ShortID(r.SessionID), r.HostName)
	}
	return nil
}

func (r *handoffRun) launch(sidecar string) error {
	var err error
	if r.launchedAt, err = clock(r.remote); err != nil {
		return err
	}
	projectNote := ""
	if r.project != nil {
		if r.setupStatus, err = prepareSetupStatus(r.remote, r.SessionID); err != nil {
			return err
		}
		projectNote = projectPrompt(r.project, r.setupStatus)
	}
	name := baseName(r.where.Root)
	prompt := handoffPrompt(Prompt{From: r.local.Name(), To: r.HostName, OldDir: r.Cwd, NewDir: r.remoteDir, Git: r.ws.Git != nil, Note: r.Note, Sidecar: sidecar, Project: projectNote, Codex: r.Codex})
	command := resumeCommand(r.SessionID, name, prompt, r.Codex)
	command[0] = expandHome(r.Host.program(r.Codex), r.where.Home)
	if r.Codex {
		r.res.note(carryCodexTrust(r.local, r.remote, r.Cwd, r.remoteDir, r.where.Root))
	} else {
		r.res.note(carryTrust(r.local, r.remote, r.Cwd, r.remoteDir))
	}
	r.res.note(clearMoved(r.remote, r.SessionID))
	if r.tab, err = r.launcher.Start(r.where.Group, tabLabel(name, r.Codex), r.remoteDir, command, true); err != nil {
		return err
	}
	var showErr error
	if r.res.ShownOn, showErr = r.launcher.Show(r.tab); showErr != nil {
		r.res.note(fmt.Errorf("showing the new tab failed: %w", showErr))
	}
	r.res.Where = r.launcher.Where(r.where.Group)
	return nil
}

func (r *handoffRun) undo(err error, startTimedOut bool) error {
	cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	r.remote = r.remote.With(cleanup).(*sh.Remote)
	r.launcher.Machine = r.remote
	if startTimedOut && !r.trustedThere() {
		err = fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that there and hand off again", err, r.remoteDir)
	}
	stray := false
	if r.tab.Pane != "" {
		if stopErr := stopSession(r.remote, r.SessionID, r.Codex); stopErr != nil {
			err = fmt.Errorf("%w; stopping it there also failed: %v", err, stopErr)
			stray = true
		}
	}
	if closeErr := r.launcher.Discard(r.tab); closeErr != nil && !sh.TabGone(closeErr) {
		err = fmt.Errorf("%w; removing the new tab also failed: %v", err, closeErr)
		stray = true
	}
	if stray {
		r.res.Stray = &Stray{Host: r.HostName, Tab: r.tab, Codex: r.Codex}
	}
	return undoSync(err, r.remote, r.ws, r.where, r.synced)
}

func (r *handoffRun) trustedThere() bool {
	if r.Codex {
		_, cfg, err := readCodexConfig(r.remote)
		return err != nil || cfg.trusts(r.remoteDir)
	}
	trusted, err := isTrusted(r.remote, r.remoteDir)
	return err != nil || trusted
}

func (r *handoffRun) finish() {
	finish, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	local, remote := r.local.With(finish), r.remote.With(finish)
	if err := settle(local, r.ws, r.Host.Exclude); err != nil {
		r.res.note(fmt.Errorf("marking this checkout as handed off failed, so handing the session back here will need a clean checkout: %w", err))
	}
	if err := recordTab(remote, r.SessionID, r.tab); err != nil {
		r.res.note(fmt.Errorf("recording the new tab failed: %w", err))
	}
	r.res.note(closeForwardsOn(remote, r.SessionID))
	if r.project != nil {
		if err := r.st.run("panes", func() error { return startProject(r.launcher, r.tab, r.where.Root, r.setupStatus, r.project) }); err != nil {
			r.res.note(errors.Unwrap(err))
		}
		if len(r.project.Ports) > 0 {
			if err := r.st.run("ports", func() error {
				return startForwards(r.SessionID, r.HostName, r.Host.SSH, r.project.Ports)
			}); err != nil {
				r.res.note(fmt.Errorf("forwarding its ports failed: %w", errors.Unwrap(err)))
			} else {
				r.res.Ports = r.project.Ports
			}
		}
	}
}
