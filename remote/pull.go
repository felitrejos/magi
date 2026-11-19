package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"
)

const (
	pullWait   = 30 * time.Minute
	pullBudget = 3 * time.Minute
)

type Pull struct {
	HostName  string
	Host      Host
	SessionID string
	Note      string
	Out       io.Writer
}

type Pulled struct {
	Result
	Session     string
	Name        string
	PromptInfo  Prompt
	Project     *Project
	ProjectRoot string
	Codex       bool
	InDesktop   bool
}

func openInDesktop(dir, sessionID, note string) error {
	if runtime.GOOS != "darwin" {
		return errors.New("Claude Desktop hand back works on macOS only")
	}
	program, err := exec.LookPath("claude")
	if err != nil {
		return err
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	notePath := returnNotePath(home, sessionID)
	if err := os.MkdirAll(filepath.Dir(notePath), 0o700); err != nil {
		return err
	}
	if err := os.WriteFile(notePath, []byte(note), 0o600); err != nil {
		return err
	}
	cmd := exec.Command("script", "-q", "/dev/null", program, "--desktop", "--resume", sessionID)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err == nil && bytes.Contains(out, []byte("Opening session")) {
		return nil
	}
	os.Remove(notePath)
	text := strings.TrimSpace(ansi.ReplaceAllString(string(out), ""))
	if err == nil {
		err = errors.New("claude --desktop didn't open it")
	}
	if text != "" {
		return fmt.Errorf("%w: %s", err, text)
	}
	return err
}

var ansi = regexp.MustCompile(`\x1b\[[0-9;?]*[a-zA-Z]|[\x00-\x08\x0e-\x1f]`)

type remoteSession struct {
	ID         string
	Transcript string
	Cwd        string
	Live       *liveSession
	Codex      bool
	Rollout    string
	Running    []int
	StaleTurn  string
	Unsure     []string
}

func (p Pull) Run() (res Pulled, err error) {
	var st stepper
	defer func() { res.Steps = st.steps }()
	if len(p.Host.LocalRoots) == 0 {
		return res, errors.New("set local_roots in the config so magi knows where projects go on this machine")
	}
	waitCtx, cancelWait := context.WithTimeout(context.Background(), pullWait)
	defer cancelWait()
	local, err := newLocal(waitCtx)
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
		if session, err = findRemoteSession(remote, p.HostName, p.SessionID); err != nil {
			return err
		}
		if err := session.refuseUnsure(remote); err != nil {
			return err
		}
		if session.Codex {
			if local.Codex() == "" {
				return fmt.Errorf("session %s is a Codex session, and codex isn't installed on %s", shortID(session.ID), local.Name())
			}
			if err := refuseOpenCodex(remote, session.ID, session.Running); err != nil {
				return err
			}
			if err := refuseOpenCodex(local, session.ID, nil); err != nil {
				return err
			}
		}
		if ws, err = readWorkspace(remote, session.Cwd, p.Host.Secrets, p.Host.Exclude); err != nil {
			return err
		}
		remoteHome, err := homeDir(remote)
		if err != nil {
			return err
		}
		if err := refuseHome(ws, remoteHome, p.HostName); err != nil {
			return err
		}
		if where, err = place(local, ws, p.Host.LocalRoots, serverGroup(ws.MainRoot, p.Host.Roots, remoteHome), p.Host.Exclude); err != nil {
			return err
		}
		if where.NewFolder != "" || ws.Git == nil {
			return nil
		}
		return checkoutAccepts(local, where.Root, ws)
	}); err != nil {
		return res, err
	}
	res.Session, res.Name, res.Codex = session.ID, baseName(where.Root), session.Codex

	if err := st.run("stop", func() error { return stopAtTurnEnd(remote, session, p.Out) }); err != nil {
		return res, err
	}

	ctx, cancel := context.WithTimeout(context.Background(), pullBudget)
	defer cancel()
	local.SetContext(ctx)
	remote.SetContext(ctx)
	stopped := func(err error) error {
		return fmt.Errorf("%w. The session is stopped on %s; to carry on there instead, run %s in %s on %s", err, p.HostName, resumeHint(session), session.Cwd, p.HostName)
	}
	if err := st.run("inspect", func() (err error) {
		ws, err = readWorkspace(remote, session.Cwd, p.Host.Secrets, p.Host.Exclude)
		return err
	}); err != nil {
		return res, stopped(err)
	}
	res.NewFolder = where.NewFolder
	localDir := filepath.Join(where.Root, ws.Subdir)
	res.FromDir, res.ToDir = session.Cwd, localDir
	synced := false
	undo := func(err error) error {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		local.SetContext(cleanup)
		return stopped(undoSync(err, local, ws, where, synced))
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
	synced = true
	var sidecar string
	if err := st.run("transcript", func() (err error) {
		if session.Codex {
			_, _, err = moveRollout(remote, session.Transcript, session.Rollout, ws.Root, local, where.Root)
			return err
		}
		sidecar, err = moveTranscript(remote, session.Transcript, ws, local, where, localDir)
		return err
	}); err != nil {
		return res, undo(err)
	}
	res.note(settle(remote, ws, p.Host.Exclude))
	_, err = sh(remote, `mv -- "$1" "$2"`, session.Transcript, session.Transcript+".handed-off-to-"+local.Name())
	res.note(err)
	res.note(markMoved(remote, session.ID, local.Name()))
	res.note(clearMoved(local, session.ID))
	if err := closeRecordedTab(remote, session.ID); err != nil {
		res.note(fmt.Errorf("closing the session's old tab on %s failed: %w", p.HostName, err))
	}
	project, err := approvedProject(local, where.Root)
	res.note(err)
	res.Project, res.ProjectRoot = project, where.Root
	res.PromptInfo = Prompt{From: p.HostName, To: local.Name(), OldDir: session.Cwd, NewDir: localDir, Git: ws.Git != nil, Note: p.Note, Sidecar: sidecar, Codex: session.Codex}
	return res, nil
}

func findRemoteSession(r *Remote, hostName, id string) (remoteSession, error) {
	if id == "" {
		last, err := lastHandoffTo(hostName)
		if err != nil {
			return remoteSession{}, err
		}
		id = last
	}
	return findSession(r, id)
}

func findSession(r Machine, id string) (remoteSession, error) {
	if !sessionIDPattern.MatchString(id) {
		return remoteSession{}, fmt.Errorf("%q is not a session id", id)
	}
	s := remoteSession{ID: id}
	out, err := sh(r, sessionScript, "find", id)
	if err != nil {
		return s, err
	}
	if s.Transcript = strings.TrimSpace(string(out)); s.Transcript == "" {
		return findCodexSession(r, id)
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
	out, err = sh(r, sessionScript, "cwd", s.Transcript)
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

const (
	exitBusy = 10
	exitGone = 11
)

func stopAtTurnEnd(r *Remote, s remoteSession, out io.Writer) error {
	stop := func() error { return stopCodex(r, s) }
	switch {
	case s.Live != nil:
		pid := strconv.Itoa(s.Live.PID)
		stop = func() error {
			_, err := sh(r, sessionScript, "stop", pid, s.ID, s.Live.since(), r.Claude())
			return err
		}
	case len(s.Running) == 0:
		return nil
	}
	told := false
	for {
		err := stop()
		if exitCode(err) != exitBusy {
			return err
		}
		if !told {
			fmt.Fprintf(out, "Waiting for the session on %s to finish its turn...\n", r.Name())
			told = true
		}
		select {
		case <-r.Context().Done():
			return fmt.Errorf("the session on %s was still busy", r.Name())
		case <-time.After(2 * time.Second):
		}
	}
}

func resumeCommand(sessionID, name, prompt string, codex bool) []string {
	if codex {
		return append([]string{"codex"}, codexResume(sessionID, prompt)...)
	}
	return []string{"claude", "--resume", sessionID, "--remote-control", name, prompt}
}

func resumeHere(dir, sessionID, name, prompt string, codex bool) error {
	args := resumeCommand(sessionID, name, prompt, codex)
	program, err := exec.LookPath(args[0])
	if err != nil {
		return err
	}
	if err := os.Chdir(dir); err != nil {
		return err
	}
	return syscall.Exec(program, args, os.Environ())
}
