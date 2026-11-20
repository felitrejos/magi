package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const (
	pullWait   = 30 * time.Minute
	pullBudget = 3 * time.Minute
)

var turnEndPoll = 2 * time.Second

var errNoSession = errors.New("no session")

type Pull struct {
	HostName  string
	Host      Host
	SessionID string
	Out       io.Writer
}

type Pulled struct {
	Result
	Session     string
	Name        string
	PromptInfo  Prompt
	Project     *Project
	ProjectRoot string
}

type remoteSession struct {
	ID         string
	Transcript string
	Cwd        string
	Live       *liveSession
}

func (p Pull) Run() (res Pulled, err error) {
	var st stepper
	defer func() { res.Steps = st.steps }()
	if len(p.Host.LocalRoots) == 0 {
		return res, errors.New("local_roots isn't set in magi's config, so magi doesn't know where projects go on this machine; call setup_machine with local_roots")
	}
	waitCtx, cancelWait := context.WithTimeout(context.Background(), pullWait)
	defer cancelWait()
	local, err := sh.NewLocal(waitCtx)
	if err != nil {
		return res, err
	}
	remote, err := newRemote(waitCtx, p.Host)
	if err != nil {
		return res, err
	}

	var session remoteSession
	var ws Workspace
	var where Placement
	if err := st.run("find", func() (err error) {
		if session, err = findSession(remote, p.SessionID); err != nil {
			return err
		}
		if ws, err = sourceWorkspace(remote, p.HostName, session.Cwd, p.Host.Secrets, p.Host.Exclude); err != nil {
			return err
		}
		if err := refuseUntested(local, remote); err != nil {
			return err
		}
		if where, err = place(local, ws, p.Host.LocalRoots, groupOf(ws.MainRoot, p.Host.Roots, remote.Home()), p.Host.Exclude); err != nil {
			return err
		}
		if where.NewFolder != "" || ws.Git == nil {
			return nil
		}
		return checkoutAccepts(local, where.Root, ws)
	}); err != nil {
		return res, err
	}
	res.Session, res.Name = session.ID, baseName(where.Root)
	localDir := filepath.Join(where.Root, ws.Subdir)
	j := &moveJournal{Session: session.ID, Kind: "pull", Host: p.HostName, Started: time.Now(), RemoteTranscript: session.Transcript, Workspace: ws, Where: where}
	if j.LocalTranscript, err = claudeTranscript(local.Home(), localDir, session.ID); err != nil {
		return res, err
	}
	if err := j.at("stop"); err != nil {
		return res, err
	}

	if err := st.run("stop", func() (err error) {
		if err := stopAtTurnEnd(remote, session, p.Out); err != nil {
			return err
		}
		j.Turns, err = turnsIn(remote, session.Transcript)
		return err
	}); err != nil {
		return res, errors.Join(err, forgetMove(session.ID))
	}

	ctx, cancel := context.WithTimeout(context.Background(), pullBudget)
	defer cancel()
	local, remote = local.With(ctx).(*sh.Local), remote.With(ctx).(*sh.Remote)
	stopped := func(err error) error {
		if session.Live == nil {
			return err
		}
		return reopen(remote, "on "+p.HostName, remote.Claude(), groupOf(ws.MainRoot, p.Host.Roots, remote.Home()), session, "Bringing this session to "+local.Name(), err)
	}
	var note string
	if err := st.run("inspect", func() (err error) {
		if ws, err = readWorkspace(remote, session.Cwd, p.Host.Secrets, p.Host.Exclude); err != nil {
			return err
		}
		j.Workspace = ws
		note, err = wantedNote(remote, session.ID)
		return err
	}); err != nil {
		return res, stopped(errors.Join(err, forgetMove(session.ID)))
	}
	if block, _, err := describeCopy(remote, local.Name(), p.Host, ws, false); err != nil {
		res.note(err)
	} else {
		fmt.Fprintln(p.Out, block)
	}
	if err := j.at("sync"); err != nil {
		return res, stopped(errors.Join(err, forgetMove(session.ID)))
	}
	res.NewFolder = where.NewFolder
	res.ToDir = localDir
	undo := func(err error) error {
		cleanup, cancel := context.WithTimeout(context.Background(), cleanupBudget)
		defer cancel()
		return stopped(j.recovery(local.With(cleanup), remote.With(cleanup), p.Host).giveBack(err))
	}

	if err := st.run("sync", func() error {
		if ws.Git == nil && where.NewFolder == "" {
			if err := ourFolderUnchanged(local, where.Root, p.Host.Exclude); err != nil {
				return err
			}
		}
		return syncProject(remote, local, ws, where, p.Host.Exclude, func() error { return pullGit(remote, local, ws, where.Root) })
	}); err != nil {
		return res, undo(err)
	}
	if err := j.at("transcript"); err != nil {
		return res, undo(err)
	}
	var sidecar string
	if err := st.run("transcript", func() (err error) {
		sidecar, err = moveTranscript(remote, session.Transcript, ws, local, where, localDir)
		return err
	}); err != nil {
		return res, undo(err)
	}
	res.note(carryTrust(remote, local, session.Cwd, localDir))
	if err := j.at("retire"); err != nil {
		return res, undo(err)
	}
	res.note(appendLog(logEntry{Time: time.Now(), Kind: "pull", Session: session.ID, From: p.HostName, To: local.Name(), FromDir: session.Cwd, ToDir: localDir}))
	res.note(closeForwards(session.ID))
	retired := retireCopy(remote, local, local.Name(), session.ID, session.Transcript, ws, p.Host.Exclude)
	if err := closeRecordedTab(remote, session.ID); err != nil {
		retired = append(retired, fmt.Errorf("closing the session's old tab on %s failed: %w", p.HostName, err))
	}
	for _, err := range retired {
		res.note(err)
	}
	if errors.Join(retired...) == nil {
		res.note(forgetMove(session.ID))
	}
	project, err := approvedProject(local, where.Root)
	res.note(err)
	res.Project, res.ProjectRoot = project, where.Root
	res.PromptInfo = Prompt{From: p.HostName, To: local.Name(), OldDir: session.Cwd, NewDir: localDir, Git: ws.Git != nil, Note: note, Sidecar: sidecar}
	return res, nil
}

func findSession(r sh.Machine, id string) (remoteSession, error) {
	if !store.SessionIDPattern.MatchString(id) {
		return remoteSession{}, fmt.Errorf("%q is not a session id", id)
	}
	s := remoteSession{ID: id}
	out, err := sh.Run(r, sessionScript, "find", id)
	if err != nil {
		return s, err
	}
	if s.Transcript = strings.TrimSpace(string(out)); s.Transcript == "" {
		return s, fmt.Errorf("%w %s on %s", errNoSession, id, r.Name())
	}
	sessions, err := liveSessions(r)
	if err != nil {
		return s, err
	}
	for i := range sessions {
		if sessions[i].SessionID == id && (s.Live == nil || sessions[i].StartedAt > s.Live.StartedAt) {
			s.Live = &sessions[i]
		}
	}
	if s.Live != nil {
		s.Cwd = s.Live.Cwd
		return s, nil
	}
	out, err = sh.Run(r, sessionScript, "cwd", s.Transcript)
	if err != nil {
		return s, err
	}
	for _, line := range slices.Backward(strings.Split(string(out), "\n")) {
		var record struct {
			Cwd string `json:"cwd"`
		}
		if json.Unmarshal([]byte(line), &record) == nil && record.Cwd != "" {
			s.Cwd = record.Cwd
			return s, nil
		}
	}
	return s, fmt.Errorf("couldn't tell which folder session %s on %s ran in", id, r.Name())
}

func stopAtTurnEnd(r *sh.Remote, s remoteSession, out io.Writer) error {
	if s.Live == nil {
		return nil
	}
	pid := strconv.Itoa(s.Live.PID)
	told := false
	for {
		_, err := sh.Run(r, sessionScript, "stop", pid, s.ID, s.Live.since(), r.Claude())
		if sh.ExitCode(err) != sh.ExitBusy {
			return err
		}
		if !told {
			fmt.Fprintf(out, "Waiting for the session on %s to finish its turn...\n", r.Name())
			told = true
		}
		select {
		case <-r.Context().Done():
			return fmt.Errorf("the session on %s was still busy", r.Name())
		case <-time.After(turnEndPoll):
		}
	}
}

func reopen(m sh.Machine, place, program, group string, s remoteSession, move string, err error) error {
	if journaled(s.ID) {
		return fmt.Errorf("%w. The session is stopped %s, and magi settles it on its own; status says where it carries on", err, place)
	}
	ctx, cancel := context.WithTimeout(context.Background(), cleanupBudget)
	defer cancel()
	name := baseName(s.Cwd)
	command := resumeCommand(s.ID, name, fmt.Sprintf("[magi] %s failed (%v), so this session carries on here.", move, err))
	command[0] = program
	if _, openErr := (sh.Tmux{Machine: m.With(ctx)}).Start(group, tabLabel(name), s.Cwd, command, false); openErr != nil {
		return fmt.Errorf("%w. The session is stopped %s, and reopening it failed (%v); the user can resume it in %s", err, place, openErr, s.Cwd)
	}
	return fmt.Errorf("%w. The session was stopped for the move, so magi reopened it %s in a tmux window", err, place)
}

func resumeCommand(sessionID, name, prompt string) []string {
	return []string{"claude", "--resume", sessionID, "--remote-control", name, prompt}
}
