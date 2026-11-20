package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"path/filepath"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const prepareBudget = 80 * time.Second

var launchBudget = 45 * time.Second

type Move struct {
	Kind     string
	HostName string
	Host     Host
	Session  remoteSession
	Note     string
	Stopped  bool
	NoTab    bool
	Out      io.Writer
	Progress func(step string)
	Retire   func() (int, error)
}

type Step struct {
	Name string
	Took time.Duration
}

type Result struct {
	Kind      string
	Session   string
	From, To  string
	Steps     []Step
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

func (r Result) Summary() string {
	var total time.Duration
	parts := make([]string, len(r.Steps))
	for i, s := range r.Steps {
		total += s.Took
		parts[i] = fmt.Sprintf("%s %.1fs", s.Name, s.Took.Seconds())
	}
	return fmt.Sprintf("%.1fs (%s)", total.Seconds(), strings.Join(parts, ", "))
}

func (r Result) Report() string {
	msg := fmt.Sprintf("Moved session %s to %s in %s", store.ShortID(r.Session), r.To, r.Summary())
	if r.Kind == "pull" {
		msg = fmt.Sprintf("Brought session %s back from %s to %s in %s", store.ShortID(r.Session), r.From, r.ToDir, r.Summary())
	}
	if r.Where != "" {
		msg += ". It's " + r.Where
	}
	if r.Link != "" {
		msg += ", and in the Claude apps at " + r.Link
	}
	if r.NewFolder != "" {
		msg += fmt.Sprintf(". %s didn't have the project yet, so it now lives at %s", r.To, r.NewFolder)
	}
	if len(r.Ports) > 0 {
		ports := make([]string, len(r.Ports))
		for i, port := range r.Ports {
			ports[i] = fmt.Sprintf("localhost:%d", port)
		}
		msg += fmt.Sprintf(". %s here now reaches it there", strings.Join(ports, ", "))
	}
	if len(r.Notes) > 0 {
		msg += ". Notes: " + r.Noted()
	}
	return msg + "."
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

type moveRun struct {
	Move
	parent     context.Context
	st         stepper
	res        Result
	local      *sh.Local
	remote     *sh.Remote
	from, to   sh.Machine
	fromRoots  []string
	toRoots    []string
	ws         Workspace
	where      Placement
	toDir      string
	project    *Project
	status     string
	tab        sh.Tab
	launchedAt time.Time
	j          *moveJournal
}

func (m Move) Run(parent context.Context) (Result, error) {
	r := &moveRun{Move: m, parent: parent, st: stepper{progress: m.Progress}, res: Result{Kind: m.Kind, Session: m.Session.ID}}
	err := r.run()
	r.res.Steps = r.st.steps
	if err != nil && r.Stopped {
		err = r.reopen(err)
	}
	return r.res, err
}

func (r *moveRun) use(ctx context.Context) {
	r.local, r.remote = r.local.With(ctx).(*sh.Local), r.remote.With(ctx).(*sh.Remote)
	r.from, r.to = r.local, r.remote
	if r.Kind == "pull" {
		r.from, r.to = r.remote, r.local
	}
}

func (r *moveRun) run() error {
	budget := prepareBudget
	if r.Kind == "pull" {
		budget = pullBudget
	}
	ctx, cancel := context.WithTimeout(r.parent, budget)
	defer cancel()
	if err := r.st.run("inspect", func() error { return r.inspect(ctx) }); err != nil {
		return err
	}
	r.toDir = filepath.Join(r.where.Root, r.ws.Subdir)
	r.res.NewFolder, r.res.ToDir = r.where.NewFolder, r.toDir
	toTranscript, err := claudeTranscript(r.to.Home(), r.toDir, r.Session.ID)
	if err != nil {
		return err
	}
	r.j = &moveJournal{Session: r.Session.ID, Kind: r.Kind, Host: r.HostName, Started: time.Now(), LocalTranscript: r.Session.Transcript, RemoteTranscript: toTranscript, Workspace: r.ws, Where: r.where}
	if r.Kind == "pull" {
		r.j.LocalTranscript, r.j.RemoteTranscript = toTranscript, r.Session.Transcript
		if err := r.j.at("stop"); err != nil {
			return err
		}
		if err := r.st.run("stop", r.stop); err != nil {
			return r.abandon(err)
		}
		ctx, cancel := context.WithTimeout(r.parent, budget)
		defer cancel()
		r.use(ctx)
		if err := r.st.run("recheck", r.recheck); err != nil {
			return r.abandon(err)
		}
	} else if r.j.Turns, err = turnsIn(r.from, r.Session.Transcript); err != nil {
		return err
	}
	if r.Out != nil {
		if block, _, err := describeCopy(r.from, r.res.To, r.Host, r.ws, r.Kind == "push"); err != nil {
			r.res.note(err)
		} else {
			fmt.Fprintln(r.Out, block)
		}
	}
	if err := r.j.at("sync"); err != nil {
		return r.abandon(err)
	}
	if err := r.st.run("sync", func() error {
		return syncProject(r.from, r.to, r.ws, r.where, r.Host.Exclude, func() error {
			if r.Kind == "pull" {
				return pullGit(r.remote, r.local, r.ws, r.where.Root)
			}
			return pushGit(r.local, r.remote, r.ws, r.where.Root)
		})
	}); err != nil {
		return r.undo(err)
	}
	if err := r.j.at("transcript"); err != nil {
		return r.undo(err)
	}
	var sidecar string
	if err := r.st.run("transcript", func() (err error) {
		sidecar, err = moveTranscript(r.from, r.Session.Transcript, r.ws, r.to, r.where, r.toDir)
		return err
	}); err != nil {
		return r.undo(err)
	}
	root := r.ws.Root
	if r.Kind == "pull" {
		root = r.where.Root
	}
	project, err := approvedProject(r.local, root)
	r.project = project
	r.res.note(err)
	if r.Kind == "push" {
		return r.push(sidecar)
	}
	if err := r.retireSource(); err != nil {
		return err
	}
	if !r.NoTab {
		r.res.note(r.open(sidecar))
	}
	return nil
}

func (r *moveRun) inspect(ctx context.Context) error {
	var err error
	if r.local, err = sh.NewLocal(ctx); err != nil {
		return err
	}
	if r.remote, err = newRemote(ctx, r.Host); err != nil {
		return err
	}
	r.use(ctx)
	r.res.From, r.res.To = r.local.Name(), r.HostName
	r.fromRoots, r.toRoots = r.Host.LocalRoots, r.Host.Roots
	if r.Kind == "pull" {
		r.res.From, r.res.To = r.res.To, r.res.From
		r.fromRoots, r.toRoots = r.toRoots, r.fromRoots
		if len(r.toRoots) == 0 {
			return errors.New("local_roots isn't set in magi's config, so magi doesn't know where projects go on this machine; call setup_machine with local_roots")
		}
		if r.Session, err = findSession(r.from, r.Session.ID); err != nil {
			return err
		}
	}
	if r.ws, err = sourceWorkspace(r.from, r.res.From, r.Session.Cwd, r.Host.Secrets, r.Host.Exclude); err != nil {
		return err
	}
	if err := refuseUntested(r.local, r.remote); err != nil {
		return err
	}
	if r.where, err = place(r.to, r.ws, r.toRoots, groupOf(r.ws.MainRoot, r.fromRoots, r.from.Home()), r.Host.Exclude); err != nil {
		return err
	}
	if r.Kind == "pull" {
		if r.where.NewFolder != "" || r.ws.Git == nil {
			return nil
		}
		return checkoutAccepts(r.to, r.where.Root, r.ws)
	}
	running, err := runningThere(r.to, r.Session.ID)
	if err != nil {
		return err
	}
	if running {
		return fmt.Errorf("session %s is already running on %s; close it there before handing it off again", store.ShortID(r.Session.ID), r.HostName)
	}
	return nil
}

func (r *moveRun) stop() error {
	wait, cancel := context.WithTimeout(r.parent, pullWait)
	defer cancel()
	from := r.remote.With(wait).(*sh.Remote)
	if err := stopAtTurnEnd(from, r.Session, r.Out); err != nil {
		return err
	}
	r.Stopped = r.Session.Live != nil
	var err error
	r.j.Turns, err = turnsIn(from, r.Session.Transcript)
	return err
}

func (r *moveRun) recheck() error {
	var err error
	if r.ws, err = readWorkspace(r.from, r.Session.Cwd, r.Host.Secrets, r.Host.Exclude); err != nil {
		return err
	}
	r.j.Workspace = r.ws
	if r.Note, err = wantedNote(r.from, r.Session.ID); err != nil || r.ws.Git != nil || r.where.NewFolder != "" {
		return err
	}
	return ourFolderUnchanged(r.to, r.where.Root, r.Host.Exclude)
}

func (r *moveRun) push(sidecar string) error {
	if err := r.open(sidecar); err != nil {
		return r.undo(err)
	}
	if err := r.j.at("finish"); err != nil {
		return r.undo(err)
	}
	r.finish()
	if err := r.parent.Err(); err != nil {
		return r.undo(fmt.Errorf("the handoff was interrupted: %w", err))
	}
	if err := r.j.at("retire"); err != nil {
		return r.undo(err)
	}
	r.res.note(appendLog(r.logEntry()))
	retirer, err := r.Retire()
	if err != nil {
		return r.undo(fmt.Errorf("closing the session here failed, so the copy on %s was stopped: %w", r.HostName, err))
	}
	if retirer > 0 {
		r.j.Retirer = retirer
		r.res.note(r.j.at("retire"))
	}
	return nil
}

func (r *moveRun) retireSource() error {
	if err := r.j.at("retire"); err != nil {
		return r.undo(err)
	}
	id := r.Session.ID
	r.res.note(appendLog(r.logEntry()))
	r.res.note(closeForwards(id))
	retired := retireCopy(r.from, r.to, r.res.To, id, r.Session.Transcript, r.ws, r.Host.Exclude)
	if err := closeRecordedTab(r.from, id); err != nil {
		retired = append(retired, fmt.Errorf("closing the session's old tab on %s failed: %w", r.HostName, err))
	}
	for _, err := range retired {
		r.res.note(err)
	}
	if errors.Join(retired...) == nil {
		r.res.note(forgetMove(id))
	}
	return nil
}

func (r *moveRun) logEntry() logEntry {
	return logEntry{Time: time.Now(), Kind: r.Kind, Session: r.Session.ID, From: r.res.From, To: r.res.To, FromDir: r.Session.Cwd, ToDir: r.toDir}
}

func (r *moveRun) open(sidecar string) error {
	ctx, cancel := context.WithTimeout(r.parent, launchBudget)
	defer cancel()
	launcher := sh.Tmux{Machine: r.to.With(ctx)}
	if err := r.st.run("launch", func() error { return r.launch(launcher, sidecar) }); err != nil {
		return err
	}
	if r.Kind == "push" {
		r.j.Tab = &r.tab
		if err := r.j.at("launched"); err != nil {
			return err
		}
	}
	if r.project != nil {
		if err := r.st.run("panes", func() error { return startProject(launcher, r.tab, r.where.Root, r.status, r.project) }); err != nil {
			r.res.note(errors.Unwrap(err))
		}
	}
	return r.st.run("start", func() error {
		var err error
		if r.res.Link, err = waitForRemoteControl(launcher.Machine, r.Session.ID, r.launchedAt); err == nil {
			return nil
		}
		check, cancel := context.WithTimeout(context.Background(), cleanupBudget)
		defer cancel()
		if trusted, trustErr := isTrusted(r.to.With(check), r.toDir); trustErr == nil && !trusted {
			return fmt.Errorf("%w: the new session was waiting for you to trust %s; accept that there", err, r.toDir)
		}
		return err
	})
}

func (r *moveRun) launch(launcher sh.Tmux, sidecar string) error {
	m := launcher.Machine
	var err error
	if r.launchedAt, err = clock(m); err != nil {
		return err
	}
	projectNote := ""
	if r.project != nil {
		if r.status, err = prepareSetupStatus(m, r.Session.ID); err != nil {
			return err
		}
		projectNote = projectPrompt(r.project, r.status)
	}
	name := baseName(r.where.Root)
	prompt := handoffPrompt(Prompt{From: r.res.From, To: r.res.To, OldDir: r.Session.Cwd, NewDir: r.toDir, Git: r.ws.Git != nil, Note: r.Note, Sidecar: sidecar, Project: projectNote})
	r.res.note(carryTrust(r.from, m, r.Session.Cwd, r.toDir))
	r.res.note(clearMoved(m, r.Session.ID))
	if r.tab, err = launcher.Start(r.where.Group, tabLabel(name), r.toDir, resumeCommand(m.Claude(), r.Session.ID, name, prompt), true); err != nil {
		return err
	}
	if err := recordTab(m, r.Session.ID, r.tab); err != nil {
		r.res.note(fmt.Errorf("recording the new tab failed: %w", err))
	}
	r.res.Where = "running " + launcher.Where(r.where.Group)
	shown, err := launcher.Show(r.tab)
	if err != nil {
		r.res.note(fmt.Errorf("showing the new tab failed: %w", err))
	}
	if len(shown) > 0 {
		r.res.Where = "open in " + strings.Join(shown, ", ")
	}
	return nil
}

func (r *moveRun) finish() {
	finish, cancel := context.WithTimeout(context.Background(), finishBudget)
	defer cancel()
	if err := settle(r.local.With(finish), r.ws, r.Host.Exclude); err != nil {
		r.res.note(fmt.Errorf("marking this checkout as handed off failed, so handing the session back here will need a clean checkout: %w", err))
	}
	r.res.note(closeForwardsOn(r.remote.With(finish), r.Session.ID))
	if r.project == nil || len(r.project.Ports) == 0 {
		return
	}
	if err := r.st.run("ports", func() error {
		return startForwards(r.Session.ID, r.HostName, r.Host.SSH, r.project.Ports)
	}); err != nil {
		r.res.note(fmt.Errorf("forwarding its ports failed: %w", errors.Unwrap(err)))
	} else {
		r.res.Ports = r.project.Ports
	}
}

func (r *moveRun) abandon(err error) error {
	if journaled(r.Session.ID) {
		return errors.Join(err, forgetMove(r.Session.ID))
	}
	return err
}

func (r *moveRun) undo(err error) error {
	cleanup, cancel := context.WithTimeout(context.Background(), cleanupBudget)
	defer cancel()
	return r.j.recovery(r.local.With(cleanup), r.remote.With(cleanup), r.Host).giveBack(err)
}

func (r *moveRun) reopen(err error) error {
	from, where, move := sh.Machine(r.local), "here", "Moving this session to "+r.HostName
	if r.Kind == "pull" {
		from, where, move = r.remote, "on "+r.HostName, "Bringing this session to "+r.res.To
	}
	if journaled(r.Session.ID) {
		return fmt.Errorf("%w. The session is stopped %s, and magi settles it on its own; status says where it carries on", err, where)
	}
	ctx, cancel := context.WithTimeout(context.Background(), cleanupBudget)
	defer cancel()
	name := baseName(r.Session.Cwd)
	command := resumeCommand(from.Claude(), r.Session.ID, name, fmt.Sprintf("[magi] %s failed (%v), so this session carries on here.", move, err))
	group := groupOf(r.Session.Cwd, r.fromRoots, from.Home())
	if _, openErr := (sh.Tmux{Machine: from.With(ctx)}).Start(group, tabLabel(name), r.Session.Cwd, command, false); openErr != nil {
		return fmt.Errorf("%w. The session is stopped %s, and reopening it failed (%v); the user can resume it in %s", err, where, openErr, r.Session.Cwd)
	}
	return fmt.Errorf("%w. The session was stopped for the move, so magi reopened it %s in a tmux window", err, where)
}

func recordTab(m sh.Machine, sessionID string, tab sh.Tab) error {
	_, err := sh.Run(m, `mkdir -p "$HOME/.magi/tabs" && printf '%s\n%s\n%s\n' "$2" "$3" "$4" > "$HOME/.magi/tabs/$1"`, sessionID, tab.Session, tab.Window, tab.Tag)
	return err
}

func closeRecordedTab(r sh.Machine, sessionID string) error {
	out, err := sh.Run(r, `f="$HOME/.magi/tabs/$1"; if [ -f "$f" ]; then cat "$f"; rm -f "$f"; fi`, sessionID)
	if err != nil || len(out) == 0 {
		return err
	}
	lines := strings.Split(string(out), "\n")
	if len(lines) < 3 {
		return fmt.Errorf("unexpected tab record for session %s on %s", sessionID, r.Name())
	}
	if err := (sh.Tmux{Machine: r}).Discard(sh.Tab{Session: lines[0], Window: lines[1], Tag: lines[2]}); err != nil && !sh.TabGone(err) {
		return err
	}
	return nil
}
