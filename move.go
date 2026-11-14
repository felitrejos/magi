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
	"strconv"
	"strings"
	"syscall"
	"time"
)

const moveHookLimit = handoffBudget + 30*time.Second

var moveHookGrace = 5 * time.Second

type moveReport struct {
	Event   string `json:"event"`
	OK      bool   `json:"ok"`
	Session string `json:"session"`
	Host    string `json:"host"`
	Message string `json:"message"`
	ToDir   string `json:"to_dir,omitempty"`
	Took    string `json:"took,omitempty"`
}

type mover struct {
	session string
	host    string
	json    bool
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

func moveResultPath(sessionID string) (string, error) {
	path, err := armedPath(sessionID)
	return strings.TrimSuffix(path, ".json") + ".result.json", err
}

func moveCommand(args []string, out io.Writer) error {
	m := mover{out: out}
	var rest []string
	for _, a := range args {
		if a == "--json" {
			m.json = true
		} else {
			rest = append(rest, a)
		}
	}
	if len(rest) != 2 {
		return m.fail(errors.New("usage: cielago move <claude-session-id> <host> [--json]"))
	}
	m.session, m.host = rest[0], rest[1]
	return m.run()
}

func (m mover) run() error {
	if !sessionIDPattern.MatchString(m.session) {
		return m.fail(fmt.Errorf("%q is not a Claude session id", m.session))
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

func lockMove(sessionID string) (func(), error) {
	path, err := armedPath(sessionID)
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(strings.TrimSuffix(path, ".json")+".lock", os.O_CREATE|os.O_RDWR, 0o600)
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
	if out, codexErr := sh(local, `find "$HOME/.codex/sessions" -name "rollout-*-$1.jsonl" -print 2>/dev/null | head -n 1`, id); codexErr == nil && len(out) > 0 {
		return s, fmt.Errorf("%s is a Codex session; cielago moves Claude Code sessions only", id)
	}
	return s, fmt.Errorf("no Claude Code session %s on %s", id, local.Name())
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
	res, err := Handoff{HostName: m.host, Host: host, SessionID: s.ID, TranscriptPath: s.Transcript, Cwd: s.Cwd}.Run()
	if err != nil {
		if stopped {
			err = fmt.Errorf("%w. The session was stopped here; to carry on here instead, run claude --resume %s in %s", err, s.ID, s.Cwd)
		}
		return m.fail(err)
	}
	finish, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	local.SetContext(finish)
	notes := []error{res.ParkNote, res.TabNote, res.SetupNote, res.ShowNote}
	if _, err := sh(local, `mv -- "$1" "$1.handed-off-to-$2"`, s.Transcript, m.host); err != nil {
		notes = append(notes, fmt.Errorf("retiring the transcript here failed: %w", err))
	}
	if err := closeLocalTab(local, s.ID); err != nil {
		notes = append(notes, fmt.Errorf("closing its old tab here failed: %w", err))
	}
	if err := appendLog(logEntry{Time: time.Now(), Kind: "push", Session: s.ID, From: res.From, To: m.host, FromDir: res.FromDir, ToDir: res.ToDir}); err != nil {
		notes = append(notes, err)
	}
	msg := fmt.Sprintf("Moved session %s to %s in %s. It's running %s", shortID(s.ID), m.host, res.Summary(), res.Where)
	if res.NewFolder != "" {
		msg += fmt.Sprintf("; %s didn't have the project yet, so it now lives at %s", m.host, res.NewFolder)
	}
	if note := errors.Join(notes...); note != nil {
		msg += fmt.Sprintf(" (%v)", strings.ReplaceAll(note.Error(), "\n", "; "))
	}
	m.report(moveReport{Event: "moved", OK: true, Message: msg + ".", ToDir: res.ToDir, Took: res.Summary()})
	return nil
}

func closeLocalTab(local *Local, sessionID string) error {
	out, err := sh(local, `f="$HOME/.cielago/tabs/$1"; if [ -f "$f" ]; then cat "$f"; rm -f "$f"; fi`, sessionID)
	if err != nil || len(out) == 0 {
		return err
	}
	lines := strings.Split(strings.TrimRight(string(out), "\n"), "\n")
	if len(lines) != 3 {
		return fmt.Errorf("unexpected tab record for session %s", sessionID)
	}
	if lines[0] == "tmux" {
		_, err = sh(local, tmuxScript, "kill-window", lines[2])
	} else {
		_, err = run(local.Context(), "rex", "window", "close", "-s", lines[1], lines[2])
	}
	if err != nil && tabGone(err) {
		return nil
	}
	return err
}

func (m mover) whenTurnEnds(s remoteSession) error {
	path, err := armedPath(s.ID)
	if err != nil {
		return m.fail(err)
	}
	result, err := moveResultPath(s.ID)
	if err != nil {
		return m.fail(err)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return m.fail(err)
	}
	os.Remove(result)
	data, err := json.Marshal(armed{Host: m.host, PID: s.Live.PID, Desktop: s.Live.Entrypoint == "claude-desktop", Notify: true})
	if err != nil {
		return m.fail(err)
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o600)
	if errors.Is(err, os.ErrExist) {
		return m.fail(errors.New("the session is already armed to move when its turn ends"))
	}
	if err != nil {
		return m.fail(err)
	}
	if _, err := f.Write(data); err != nil || f.Close() != nil {
		os.Remove(path)
		return m.fail(fmt.Errorf("arming the session failed: %v", err))
	}
	m.report(moveReport{Event: "waiting", OK: true, Message: fmt.Sprintf("Session %s is in the middle of a turn; it moves to %s as soon as the turn ends.", shortID(s.ID), m.host)})

	interrupted := make(chan os.Signal, 1)
	signal.Notify(interrupted, os.Interrupt, syscall.SIGTERM, syscall.SIGHUP)
	defer signal.Stop(interrupted)
	local, err := newLocal(context.Background())
	if err != nil {
		os.Remove(path)
		return m.fail(err)
	}
	var idleSince, takenAt time.Time
	for {
		if data, err := os.ReadFile(result); err == nil {
			os.Remove(result)
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
		_, armErr := os.Stat(path)
		armedStill := armErr == nil
		if !armedStill && takenAt.IsZero() {
			takenAt = time.Now()
		}
		if !takenAt.IsZero() && time.Since(takenAt) > moveHookLimit {
			return m.fail(errors.New("the Stop hook took the handoff but never reported back; check cielago status"))
		}
		if armedStill {
			alive, err := isClaudeProcess(local, *s.Live)
			if err == nil && !alive {
				os.Remove(path)
				return m.fail(errors.New("the session ended before its turn did, so it didn't move"))
			}
			if status := sessionStatus(local, s.Live.PID); status == "idle" {
				if idleSince.IsZero() {
					idleSince = time.Now()
				} else if time.Since(idleSince) > moveHookGrace {
					os.Remove(path)
					return m.fail(errors.New("the session's turn ended but cielago's Stop hook didn't run; install the cielago Claude Code plugin on this machine"))
				}
			} else {
				idleSince = time.Time{}
			}
		}
		select {
		case <-interrupted:
			if os.Remove(path) == nil {
				return m.fail(errors.New("cancelled; the session keeps running here"))
			}
			return m.fail(errors.New("cancelled, but the handoff had already started; check cielago status"))
		case <-time.After(300 * time.Millisecond):
		}
	}
}

func sessionStatus(m Machine, pid int) string {
	out, err := sh(m, `cat "$HOME/.claude/sessions/$1.json" 2>/dev/null`, strconv.Itoa(pid))
	if err != nil {
		return ""
	}
	var s liveSession
	if json.Unmarshal(out, &s) != nil {
		return ""
	}
	return s.Status
}

func writeMoveResult(sessionID string, r moveReport) error {
	path, err := moveResultPath(sessionID)
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
