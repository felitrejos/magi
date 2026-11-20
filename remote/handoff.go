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

const prepareBudget = 80 * time.Second

var launchBudget = 45 * time.Second

type Handoff struct {
	HostName       string
	Host           Host
	SessionID      string
	TranscriptPath string
	Cwd            string
	Note           string
	Progress       func(step string)
	Show           func(block string)
	Retire         func() (int, error)
}

type Step struct {
	Name string
	Took time.Duration
}

type Result struct {
	Steps     []Step
	ShownOn   []string
	Where     string
	Link      string
	Notes     []error
	Ports     []int
	NewFolder string
	ToDir     string
}

func tabLabel(name string) string {
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
	launchedAt  time.Time
	setupStatus string
	j           *moveJournal
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
	if r.Show != nil {
		if block, _, err := describeCopy(r.local, r.HostName, r.Host, r.ws, true); err != nil {
			r.res.note(err)
		} else {
			r.Show(block)
		}
	}
	r.remoteDir = filepath.Join(r.where.Root, r.ws.Subdir)
	r.res.NewFolder, r.res.ToDir = r.where.NewFolder, r.remoteDir
	r.launcher = sh.Tmux{Machine: r.remote}
	r.j = &moveJournal{Session: r.SessionID, Kind: "push", Host: r.HostName, Started: time.Now(), LocalTranscript: r.TranscriptPath, Workspace: r.ws, Where: r.where}
	if r.j.Turns, err = turnsIn(r.local, r.TranscriptPath); err != nil {
		return err
	}
	if r.j.RemoteTranscript, err = claudeTranscript(r.remote.Home(), r.remoteDir, r.SessionID); err != nil {
		return err
	}
	if err := r.j.at("sync"); err != nil {
		return err
	}

	if err := r.st.run("sync", func() error {
		return syncProject(r.local, r.remote, r.ws, r.where, r.Host.Exclude, func() error { return pushGit(r.local, r.remote, r.ws, r.where.Root) })
	}); err != nil {
		return r.undo(err, false)
	}
	if err := r.j.at("transcript"); err != nil {
		return r.undo(err, false)
	}
	var sidecar string
	if err := r.st.run("transcript", func() (err error) {
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
		var err error
		r.res.Link, err = waitForRemoteControl(remote, r.SessionID, r.launchedAt)
		return err
	}); err != nil {
		return r.undo(err, true)
	}
	if err := r.j.at("finish"); err != nil {
		return r.undo(err, false)
	}
	r.finish()
	if err := r.parent.Err(); err != nil {
		return r.undo(fmt.Errorf("the handoff was interrupted: %w", err), false)
	}
	if err := r.j.at("retire"); err != nil {
		return r.undo(err, false)
	}
	r.res.note(appendLog(logEntry{Time: time.Now(), Kind: "push", Session: r.SessionID, From: r.local.Name(), To: r.HostName, FromDir: r.Cwd, ToDir: r.remoteDir}))
	retirer, err := r.Retire()
	if err != nil {
		return r.undo(fmt.Errorf("closing the session here failed, so the copy on %s was stopped: %w", r.HostName, err), false)
	}
	if retirer > 0 {
		r.j.Retirer = retirer
		r.res.note(r.j.at("retire"))
	}
	return nil
}

func (r *handoffRun) inspect(ctx context.Context) error {
	var err error
	if r.ws, err = sourceWorkspace(r.local, r.local.Name(), r.Cwd, r.Host.Secrets, r.Host.Exclude); err != nil {
		return err
	}
	if r.remote, err = newRemote(ctx, r.Host); err != nil {
		return err
	}
	if r.where, err = place(r.remote, r.ws, r.Host.Roots, groupOf(r.ws.MainRoot, r.Host.LocalRoots, r.local.Home()), r.Host.Exclude); err != nil {
		return err
	}
	if err := refuseUntested(r.local, r.remote); err != nil {
		return err
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
	prompt := handoffPrompt(Prompt{From: r.local.Name(), To: r.HostName, OldDir: r.Cwd, NewDir: r.remoteDir, Git: r.ws.Git != nil, Note: r.Note, Sidecar: sidecar, Project: projectNote})
	command := resumeCommand(r.SessionID, name, prompt)
	command[0] = r.remote.Claude()
	r.res.note(carryTrust(r.local, r.remote, r.Cwd, r.remoteDir))
	r.res.note(clearMoved(r.remote, r.SessionID))
	if r.tab, err = r.launcher.Start(r.where.Group, tabLabel(name), r.remoteDir, command, true); err != nil {
		return err
	}
	r.j.Tab = &r.tab
	if err := r.j.at("launched"); err != nil {
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
	cleanup, cancel := context.WithTimeout(context.Background(), cleanupBudget)
	defer cancel()
	r.remote = r.remote.With(cleanup).(*sh.Remote)
	if startTimedOut && !r.trustedThere() {
		err = fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that there and hand off again", err, r.remoteDir)
	}
	return r.j.recovery(r.local.With(cleanup), r.remote, r.Host).giveBack(err)
}

func (r *handoffRun) trustedThere() bool {
	trusted, err := isTrusted(r.remote, r.remoteDir)
	return err != nil || trusted
}

func (r *handoffRun) finish() {
	finish, cancel := context.WithTimeout(context.Background(), finishBudget)
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
