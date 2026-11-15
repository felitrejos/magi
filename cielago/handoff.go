package main

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"time"
)

const handoffBudget = 110 * time.Second

type Handoff struct {
	HostName       string
	Host           Host
	SessionID      string
	TranscriptPath string
	Cwd            string
	Note           string
	Stay           bool
	Progress       func(step string)
}

type Step struct {
	Name string
	Took time.Duration
}

type Result struct {
	Steps     []Step
	ShownOn   []string
	Where     string
	ShowNote  error
	TabNote   error
	SetupNote error
	ParkNote  error
	TrustNote error
	NewFolder string
	From      string
	FromDir   string
	ToDir     string
}

func (r Result) Total() time.Duration {
	var total time.Duration
	for _, s := range r.Steps {
		total += s.Took
	}
	return total
}

func (r Result) Summary() string {
	var total time.Duration
	parts := make([]string, len(r.Steps))
	for i, s := range r.Steps {
		total += s.Took
		parts[i] = fmt.Sprintf("%s %.1fs", s.Name, s.Took.Seconds())
	}
	return fmt.Sprintf("%.1fs (%s)", total.Seconds(), strings.Join(parts, ", "))
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

func (h Handoff) Run() (res Result, err error) {
	ctx, cancel := context.WithTimeout(context.Background(), handoffBudget)
	defer cancel()
	st := stepper{progress: h.Progress}
	defer func() { res.Steps = st.steps }()

	local, err := newLocal(ctx)
	if err != nil {
		return res, err
	}
	var remote *Remote
	var ws Workspace
	var where Placement
	if err := st.run("inspect", func() (err error) {
		if ws, err = readWorkspace(local, h.Cwd, h.Host.Secrets, h.Host.Exclude); err != nil {
			return err
		}
		home, err := homeDir(local)
		if err != nil {
			return err
		}
		if err := refuseHome(ws, home, local.Name()); err != nil {
			return err
		}
		if remote, err = newRemote(ctx, h.Host.SSH); err != nil {
			return err
		}
		group, err := localGroup(ws.MainRoot, h.Host.LocalRoots)
		if err != nil {
			return err
		}
		where, err = place(remote, ws, h.Host.Roots, group, h.Host.Exclude)
		return err
	}); err != nil {
		return res, err
	}
	project, err := approvedProject(local, ws.Root)
	if err != nil {
		res.SetupNote = err
	}
	res.NewFolder = where.NewFolder
	res.From = local.Name()
	res.FromDir = h.Cwd
	remoteDir := filepath.Join(where.Root, ws.Subdir)
	res.ToDir = remoteDir

	launcher := newLauncher(h.Host, remote)
	var tab Tab
	synced := false
	undo := func(err error, startTimedOut bool) error {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		remote.SetContext(cleanup)
		if startTimedOut {
			if trusted, trustErr := isTrusted(remote, remoteDir); trustErr == nil && !trusted {
				err = fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that there and hand off again", err, remoteDir)
			}
		}
		if tab.Pane != "" {
			if stopErr := stopSession(remote, h.SessionID); stopErr != nil {
				err = fmt.Errorf("%w; stopping it there also failed: %v", err, stopErr)
			}
		}
		if closeErr := launcher.Discard(tab); closeErr != nil {
			err = fmt.Errorf("%w; removing the new tab also failed: %v", err, closeErr)
		}
		return undoSync(err, remote, ws, where, synced)
	}

	if err := st.run("sync", func() error {
		return syncProject(local, remote, ws, where, h.Host.Exclude, func() error { return pushGit(local, remote, ws, where.Root) })
	}); err != nil {
		return res, undo(err, false)
	}
	synced = true

	sidecar := ""
	if err := st.run("transcript", func() (err error) {
		sidecar, err = moveTranscript(local, h.TranscriptPath, ws, remote, where, remoteDir)
		return err
	}); err != nil {
		return res, undo(err, false)
	}

	var launchedAt time.Time
	var setupStatus string
	name := baseName(where.Root)
	if err := st.run("launch", func() (err error) {
		if launchedAt, err = clock(remote); err != nil {
			return err
		}
		projectNote := ""
		if project != nil {
			if setupStatus, err = prepareSetupStatus(remote, h.SessionID); err != nil {
				return err
			}
			projectNote = projectPrompt(project, setupStatus)
		}
		res.TrustNote = carryTrust(local, remote, h.Cwd, remoteDir)
		prompt := handoffPrompt(Prompt{From: local.Name(), To: h.HostName, OldDir: h.Cwd, NewDir: remoteDir, Git: ws.Git != nil, Note: h.Note, Sidecar: sidecar, Project: projectNote})
		command := []string{expandHome(h.Host.Claude, where.Home), "--resume", h.SessionID, "--remote-control", name, prompt}
		if tab, err = launcher.Start(where.Group, claudeTabLabel(name), remoteDir, command, !h.Stay); err != nil {
			return err
		}
		if !h.Stay {
			res.ShownOn, res.ShowNote = launcher.Show(tab)
		}
		res.Where = launcher.Where(where.Group)
		return nil
	}); err != nil {
		return res, undo(err, false)
	}

	if err := st.run("start", func() error { return waitForRemoteControl(remote, h.SessionID, launchedAt) }); err != nil {
		return res, undo(err, true)
	}

	finish, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	local.SetContext(finish)
	remote.SetContext(finish)
	res.ParkNote = errors.Join(settle(local, ws, h.Host.Exclude), recordTab(remote, h.SessionID, h.Host.Launcher, tab))
	if project != nil {
		if err := st.run("panes", func() error { return startProject(remote, launcher, tab, where.Root, setupStatus, project) }); err != nil {
			res.SetupNote = errors.Unwrap(err)
		}
	}
	if h.Host.NewTabCommand != "" {
		res.TabNote = st.run("tab", func() error {
			return launcher.OpenProjectTab(tab, name, where.Root, strings.ReplaceAll(h.Host.NewTabCommand, "{repo}", quote(name)))
		})
	}
	return res, nil
}
