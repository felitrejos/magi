package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"
)

const (
	hookLimit     = 200 * time.Second
	moveHookLimit = hookLimit + 10*time.Second
)

var moveHookGrace = 5 * time.Second

type moveReport struct {
	Event   string `json:"event"`
	OK      bool   `json:"ok"`
	Session string `json:"session"`
	Host    string `json:"host"`
	Message string `json:"message"`
	ToDir   string `json:"to_dir,omitempty"`
	Took    string `json:"took,omitempty"`
	Step    string `json:"step,omitempty"`
	Seconds string `json:"seconds,omitempty"`
	Ports   []int  `json:"ports,omitempty"`
}

type mover struct {
	session string
	host    string
	json    bool
	stay    bool
	out     io.Writer
}

func (m mover) report(r moveReport) {
	r.Session, r.Host = m.session, m.host
	if m.json {
		json.NewEncoder(m.out).Encode(r)
		return
	}
	fmt.Fprintln(m.out, r.Message)
}

func (m mover) fail(err error) error {
	m.report(moveReport{Event: "failed", Message: err.Error()})
	return errReported
}

var errReported = errors.New("reported")

func moveCommand(args []string, out io.Writer) error {
	m := mover{out: out}
	var rest []string
	for _, a := range args {
		switch a {
		case "--json":
			m.json = true
		case "--stay":
			m.stay = true
		default:
			rest = append(rest, a)
		}
	}
	if len(rest) != 2 {
		return m.fail(errors.New("usage: magi move <session-id> <host> [--json] [--stay]"))
	}
	m.session, m.host = rest[0], rest[1]
	return m.run()
}

func (m mover) run() error {
	if !sessionIDPattern.MatchString(m.session) {
		return m.fail(fmt.Errorf("%q is not a session id", m.session))
	}
	host, err := loadHost(m.host)
	if err != nil {
		return m.fail(err)
	}
	unlock, err := lockMove(m.session)
	if err != nil {
		return m.fail(err)
	}
	defer unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	local, err := newLocal(ctx)
	if err != nil {
		return m.fail(err)
	}
	s, err := findLocalSession(local, m.session)
	if err != nil {
		return m.fail(err)
	}
	if s.Codex {
		return m.codex(local, host, s)
	}
	if s.Live != nil {
		running, err := isClaudeProcess(local, *s.Live)
		if err != nil {
			return m.fail(err)
		}
		if !running {
			s.Live = nil
		}
	}
	if s.Live != nil && (s.Live.Status == "busy" || s.Live.Status == "waiting") {
		return m.whenTurnEnds(s)
	}
	stopped := false
	if s.Live != nil {
		_, err := sh(local, sessionScript, "stop", strconv.Itoa(s.Live.PID), s.ID, s.Live.since())
		if exitCode(err) == exitBusy {
			return m.whenTurnEnds(s)
		}
		if err != nil {
			return m.fail(err)
		}
		stopped = true
	}
	return m.now(local, host, s, stopped)
}

func (m mover) codex(local *Local, host Host, s remoteSession) error {
	if host.Codex == "" {
		return m.fail(fmt.Errorf("session %s is a Codex session; set codex to the path of codex on %s under [hosts.%s] in magi's config to move it there", shortID(s.ID), m.host, tomlKey(m.host)))
	}
	if err := s.refuseUnsure(local); err != nil {
		return m.fail(err)
	}
	if err := refuseOpenCodex(local, s.ID, s.Running); err != nil {
		return m.fail(err)
	}
	remote, err := newRemote(local.Context(), host.SSH)
	if err != nil {
		return m.fail(err)
	}
	if err := refuseOpenCodex(remote, s.ID, nil); err != nil {
		return m.fail(err)
	}
	if len(s.Running) == 0 {
		return m.now(local, host, s, false)
	}
	local.SetContext(context.Background())
	interrupted := make(chan os.Signal, 1)
	signal.Notify(interrupted, os.Interrupt, syscall.SIGTERM, syscall.SIGHUP)
	defer signal.Stop(interrupted)
	for told := false; ; told = true {
		err := stopCodex(local, s)
		if exitCode(err) != exitBusy {
			if err != nil {
				return m.fail(err)
			}
			return m.now(local, host, s, true)
		}
		if !told {
			m.report(moveReport{Event: "waiting", OK: true, Message: fmt.Sprintf("Session %s is in the middle of a turn; it moves to %s as soon as the turn ends.", shortID(s.ID), m.host)})
		}
		if !slices.ContainsFunc(s.Running, func(pid int) bool { return syscall.Kill(pid, 0) == nil }) {
			return m.fail(errors.New("the session ended before its turn did, so it didn't move"))
		}
		select {
		case <-interrupted:
			return m.fail(errors.New("cancelled; the session keeps running here"))
		case <-time.After(500 * time.Millisecond):
		}
	}
}

func lockMove(sessionID string) (func(), error) {
	path, err := armedPath(sessionID, ".lock")
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		f.Close()
		return nil, fmt.Errorf("session %s is already being moved", shortID(sessionID))
	}
	return func() { f.Close() }, nil
}

func findLocalSession(local *Local, id string) (remoteSession, error) {
	s, err := findSession(local, id)
	if err == nil || !strings.HasPrefix(err.Error(), "no session ") {
		return s, err
	}
	if live, _ := liveSessions(local); slices.ContainsFunc(live, func(l liveSession) bool { return l.SessionID == id }) {
		return s, fmt.Errorf("session %s hasn't started a conversation yet, so there's nothing to move", shortID(id))
	}
	return s, fmt.Errorf("no Claude Code or Codex session %s on %s", id, local.Name())
}

func isClaudeProcess(m Machine, s liveSession) (bool, error) {
	_, err := sh(m, sessionScript, "alive", strconv.Itoa(s.PID), s.since())
	if exitCode(err) == 1 {
		return false, nil
	}
	return err == nil, err
}

func (m mover) now(local *Local, host Host, s remoteSession, stopped bool) error {
	m.report(moveReport{Event: "moving", OK: true, Message: fmt.Sprintf("Moving session %s to %s...", shortID(s.ID), m.host)})
	progress := func(step string) {
		m.report(moveReport{Event: "step", OK: true, Step: step, Message: fmt.Sprintf("Moving session %s to %s: %s", shortID(s.ID), m.host, step)})
	}
	res, err := Handoff{HostName: m.host, Host: host, SessionID: s.ID, TranscriptPath: s.Transcript, Cwd: s.Cwd, Stay: m.stay, Progress: progress, Codex: s.Codex, Rollout: s.Rollout}.Run(context.Background())
	if res.Stray != nil {
		res.note(saveStray(s.ID, *res.Stray))
	}
	if err != nil {
		if stopped {
			err = fmt.Errorf("%w. The session was stopped here; to carry on here instead, run %s in %s", err, resumeHint(s), s.Cwd)
		}
		return m.fail(err)
	}
	finish, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	local.SetContext(finish)
	if _, err := sh(local, retireScript, "", "0", s.Transcript, m.host, "", "", s.ID, ""); err != nil && !tabGone(err) {
		res.note(fmt.Errorf("retiring the session here failed: %w", err))
	}
	res.note(appendLog(logEntry{Time: time.Now(), Kind: "push", Session: s.ID, From: res.From, To: m.host, FromDir: res.FromDir, ToDir: res.ToDir}))
	msg := fmt.Sprintf("Moved session %s to %s in %s. It's running %s", shortID(s.ID), m.host, res.Summary(), res.Where)
	if res.NewFolder != "" {
		msg += fmt.Sprintf("; %s didn't have the project yet, so it now lives at %s", m.host, res.NewFolder)
	}
	if forwarded := res.Forwarded(); forwarded != "" {
		msg += "; " + forwarded
	}
	if len(res.Notes) > 0 {
		msg += fmt.Sprintf(" (%s)", res.Noted())
	}
	m.report(moveReport{Event: "moved", OK: true, Message: msg + ".", ToDir: res.ToDir, Took: res.Summary(), Seconds: fmt.Sprintf("%.1f", res.Total().Seconds()), Ports: res.Ports})
	return nil
}

func (m mover) whenTurnEnds(s remoteSession) error {
	path, err := armedPath(s.ID, ".json")
	if err != nil {
		return m.fail(err)
	}
	result, err := armedPath(s.ID, ".result.json")
	if err != nil {
		return m.fail(err)
	}
	if err := os.Remove(result); err != nil && !errors.Is(err, os.ErrNotExist) {
		return m.fail(err)
	}
	err = writeArmed(path, armed{Host: m.host, PID: s.Live.PID, Desktop: s.Live.Entrypoint == "claude-desktop", Notify: true, Stay: m.stay}, os.O_EXCL)
	if errors.Is(err, os.ErrExist) {
		return m.fail(errors.New("the session is already armed to move when its turn ends"))
	}
	disarm := func(err error) error {
		if rmErr := os.Remove(path); rmErr != nil && !errors.Is(rmErr, os.ErrNotExist) {
			err = fmt.Errorf("%w; removing %s also failed: %v", err, path, rmErr)
		}
		return m.fail(err)
	}
	if err != nil {
		return disarm(fmt.Errorf("arming the session failed: %w", err))
	}
	m.report(moveReport{Event: "waiting", OK: true, Message: fmt.Sprintf("Session %s is in the middle of a turn; it moves to %s as soon as the turn ends.", shortID(s.ID), m.host)})

	interrupted := make(chan os.Signal, 1)
	signal.Notify(interrupted, os.Interrupt, syscall.SIGTERM, syscall.SIGHUP)
	defer signal.Stop(interrupted)
	local, err := newLocal(context.Background())
	if err != nil {
		return disarm(err)
	}
	var idleSince, takenAt time.Time
	for {
		data, err := os.ReadFile(result)
		if err == nil {
			if err := os.Remove(result); err != nil {
				return m.fail(err)
			}
			var r moveReport
			if err := json.Unmarshal(data, &r); err != nil {
				return m.fail(fmt.Errorf("unreadable result from the Stop hook: %w", err))
			}
			m.report(r)
			if !r.OK {
				return errReported
			}
			return nil
		}
		if !errors.Is(err, os.ErrNotExist) {
			return m.fail(err)
		}
		armedStill, err := exists(path)
		if err != nil {
			return m.fail(err)
		}
		if !armedStill && takenAt.IsZero() {
			takenAt = time.Now()
		}
		if !takenAt.IsZero() && time.Since(takenAt) > moveHookLimit {
			return m.fail(errors.New("the Stop hook took the handoff but never reported back; check magi status"))
		}
		if armedStill {
			alive, err := isClaudeProcess(local, *s.Live)
			if err != nil {
				return disarm(err)
			}
			if !alive {
				return disarm(errors.New("the session ended before its turn did, so it didn't move"))
			}
			live, _, err := readLiveSession(s.Live.PID)
			if err != nil && !halfWritten(err) {
				return disarm(err)
			}
			if err != nil || live.Status != "idle" {
				idleSince = time.Time{}
			} else if idleSince.IsZero() {
				idleSince = time.Now()
			} else if time.Since(idleSince) > moveHookGrace {
				return disarm(errors.New("the session's turn ended but magi's Stop hook didn't run; install the magi Claude Code plugin on this machine"))
			}
		}
		select {
		case <-interrupted:
			if os.Remove(path) == nil {
				return m.fail(errors.New("cancelled; the session keeps running here"))
			}
			return m.fail(errors.New("cancelled, but the handoff had already started; check magi status"))
		case <-time.After(300 * time.Millisecond):
		}
	}
}

func exists(path string) (bool, error) {
	_, err := os.Stat(path)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	return err == nil, err
}

func readLiveSession(pid int) (liveSession, bool, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return liveSession{}, false, err
	}
	data, err := os.ReadFile(filepath.Join(home, ".claude", "sessions", strconv.Itoa(pid)+".json"))
	if errors.Is(err, os.ErrNotExist) {
		return liveSession{}, false, nil
	}
	if err != nil {
		return liveSession{}, false, err
	}
	var s liveSession
	if err := json.Unmarshal(data, &s); err != nil {
		return liveSession{}, false, err
	}
	return s, true, nil
}

func halfWritten(err error) bool {
	var syntax *json.SyntaxError
	return errors.As(err, &syntax)
}

func writeMoveResult(sessionID string, r moveReport) error {
	path, err := armedPath(sessionID, ".result.json")
	if err != nil {
		return err
	}
	data, err := json.Marshal(r)
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
