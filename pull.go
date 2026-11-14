package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
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
	Out       io.Writer
}

type Pulled struct {
	Result
	Session string
	Name    string
	Prompt  string
}

type remoteSession struct {
	ID         string
	Transcript string
	Cwd        string
	Live       *liveSession
}

func (p Pull) Run() (Pulled, error) {
	var res Pulled
	var st stepper
	if len(p.Host.LocalRoots) == 0 {
		return res, errors.New("set local_roots in the config so cielago knows where projects go on this machine")
	}
	waitCtx, cancelWait := context.WithTimeout(context.Background(), pullWait)
	defer cancelWait()
	local, err := newLocal(waitCtx)
	if err != nil {
		return res, err
	}
	remote, err := newRemote(waitCtx, p.Host.SSH)
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
		remoteHome, err := homeDir(remote)
		if err != nil {
			return err
		}
		if ws, err = readWorkspace(remote, session.Cwd, p.Host.Secrets); err != nil {
			return err
		}
		if ws.Git == nil && (ws.Root == remoteHome || ws.Root == "/") {
			return fmt.Errorf("%s is a whole home folder on %s, not a project", ws.Root, p.HostName)
		}
		if where, err = place(local, ws, p.Host.LocalRoots, serverGroup(ws.MainRoot, p.Host.Roots, remoteHome), p.Host.Exclude); err != nil {
			return err
		}
		if where.NewFolder != "" || ws.Git == nil {
			return nil
		}
		return checkoutAccepts(local, where.Root, ws)
	}); err != nil {
		res.Steps = st.steps
		return res, err
	}
	res.Session, res.Name = session.ID, baseName(where.Root)

	if err := st.run("stop", func() error { return stopAtTurnEnd(remote, session, p.Out) }); err != nil {
		res.Steps = st.steps
		return res, err
	}

	ctx, cancel := context.WithTimeout(context.Background(), pullBudget)
	defer cancel()
	local.SetContext(ctx)
	remote.SetContext(ctx)
	stopped := func(err error) error {
		return fmt.Errorf("%w. The session is stopped on %s; to carry on there instead, run claude --resume %s in %s on %s", err, p.HostName, session.ID, session.Cwd, p.HostName)
	}
	if err := st.run("inspect", func() (err error) {
		ws, err = readWorkspace(remote, session.Cwd, p.Host.Secrets)
		return err
	}); err != nil {
		res.Steps = st.steps
		return res, stopped(err)
	}
	res.NewFolder = where.NewFolder
	localDir := filepath.Join(where.Root, ws.Subdir)
	res.From, res.FromDir, res.ToDir = p.HostName, session.Cwd, localDir
	synced := false
	undo := func(err error) error {
		cleanup, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		local.SetContext(cleanup)
		switch {
		case where.NewFolder != "":
			err = withFolderUndo(err, local, where, local.Name())
		case synced && ws.Git != nil:
			if parkErr := park(local, where.Root, ws); parkErr != nil {
				err = fmt.Errorf("%w; marking %s as handed off also failed: %v", err, where.Root, parkErr)
			}
		}
		return stopped(err)
	}

	if err := st.run("sync", func() error {
		if ws.Git == nil {
			if where.NewFolder == "" {
				f, err := folderState(local, where.Root)
				if err != nil {
					return err
				}
				if err := folderUnchanged(local, where.Root, f.Recorded, p.Host.Exclude); err != nil {
					return err
				}
			}
			if err := mirrorFolder(remote, ws.Root, local, where.Root, p.Host.Exclude); err != nil {
				return err
			}
			return registerFolder(local, where.Root, p.Host.Exclude)
		}
		if where.NewFolder != "" {
			if err := createClone(local, where.NewFolder, ws.Git.Origin); err != nil {
				return err
			}
		} else if err := checkoutAccepts(local, where.Root, ws); err != nil {
			return err
		}
		return pullGit(remote, local, ws, where.Root)
	}); err != nil {
		res.Steps = st.steps
		return res, undo(err)
	}
	synced = true
	var sidecar string
	if err := st.run("transcript", func() (err error) {
		sidecar, err = moveTranscript(remote, session.Transcript, ws, local, where, localDir)
		return err
	}); err != nil {
		res.Steps = st.steps
		return res, undo(err)
	}
	res.ParkNote = settle(remote, ws, p.Host.Exclude)
	if _, err := sh(remote, "mv -- "+quote(session.Transcript)+" "+quote(session.Transcript+".handed-off-to-"+local.Name())); err != nil {
		res.ParkNote = errors.Join(res.ParkNote, err)
	}
	res.Steps = st.steps
	res.Prompt = handoffPrompt(Prompt{From: p.HostName, To: local.Name(), OldDir: session.Cwd, NewDir: localDir, Git: ws.Git != nil, Sidecar: sidecar})
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
	if !sessionIDPattern.MatchString(id) {
		return remoteSession{}, fmt.Errorf("%q is not a Claude session id", id)
	}
	s := remoteSession{ID: id}
	out, err := sh(r, `ls -t "$HOME"/.claude/projects/*/`+quote(id+".jsonl")+` 2>/dev/null | head -n 1`)
	if err != nil {
		return s, err
	}
	if s.Transcript = strings.TrimSpace(string(out)); s.Transcript == "" {
		return s, fmt.Errorf("no session %s on %s", id, r.Name())
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
	cwd, err := sh(r, "tail -n 400 "+quote(s.Transcript)+` | grep -o '"cwd":"[^"]*"' | tail -n 1`)
	if err != nil {
		return s, err
	}
	var record struct {
		Cwd string `json:"cwd"`
	}
	if json.Unmarshal([]byte("{"+strings.TrimSpace(string(cwd))+"}"), &record) != nil || record.Cwd == "" {
		return s, fmt.Errorf("couldn't tell which folder session %s on %s ran in", id, r.Name())
	}
	s.Cwd = record.Cwd
	return s, nil
}

const exitBusy = 10

func stopAtTurnEnd(r *Remote, s remoteSession, out io.Writer) error {
	if s.Live == nil {
		return nil
	}
	pid := strconv.Itoa(s.Live.PID)
	file := `"$HOME"/.claude/sessions/` + pid + ".json"
	stop := `if ! kill -0 ` + pid + ` 2>/dev/null || ! ps -o args= -p ` + pid + ` | grep -q claude; then exit 0; fi; ` +
		`if [ -f ` + file + ` ] && grep -q '"status": *"busy"' ` + file + `; then exit ` + strconv.Itoa(exitBusy) + `; fi; ` +
		`kill -TERM ` + pid + `; for i in $(seq 1 50); do kill -0 ` + pid + ` 2>/dev/null || break; sleep 0.2; done; ` +
		`if kill -0 ` + pid + ` 2>/dev/null; then echo "the session on this machine didn't stop" >&2; exit 1; fi; ` +
		`if pgrep -f -- ` + quote("[-]-resume "+s.ID) + ` >/dev/null; then echo "another process is still running this session" >&2; exit 1; fi`
	told := false
	for {
		_, err := sh(r, stop)
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

func resumeHere(dir, sessionID, name, prompt string) error {
	claude, err := exec.LookPath("claude")
	if err != nil {
		return err
	}
	if err := os.Chdir(dir); err != nil {
		return err
	}
	return syscall.Exec(claude, []string{"claude", "--resume", sessionID, "--remote-control", name, prompt}, os.Environ())
}
