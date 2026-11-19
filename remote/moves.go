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
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const recoverBudget = 15 * time.Second

type moveJournal struct {
	Session          string    `json:"-"`
	Kind             string    `json:"kind"`
	Host             string    `json:"host"`
	Codex            bool      `json:"codex"`
	Started          time.Time `json:"started"`
	Step             string    `json:"step"`
	Undone           bool      `json:"undone,omitempty"`
	Turns            int       `json:"turns"`
	LocalTranscript  string    `json:"local_transcript"`
	RemoteTranscript string    `json:"remote_transcript"`
	Workspace        Workspace `json:"workspace"`
	Where            Placement `json:"where"`
	Tab              *sh.Tab   `json:"tab,omitempty"`
	Retirer          int       `json:"retirer,omitempty"`
}

func movePath(id string) (string, error) {
	return store.SessionPath("moves", id, ".json")
}

func (j *moveJournal) at(step string) error {
	j.Step = step
	path, err := movePath(j.Session)
	if err != nil {
		return err
	}
	return store.WriteJSON(path, j)
}

func forgetMove(id string) error {
	path, err := movePath(id)
	if err != nil {
		return err
	}
	return os.Remove(path)
}

func (j *moveJournal) reached(steps ...string) bool {
	return slices.Contains(steps, j.Step)
}

func transcriptOn(m sh.Machine, home, dir, id string, codex bool, rollout string) (string, error) {
	if !codex {
		return claudeTranscript(home, dir, id)
	}
	out, err := sh.Run(m, codexScript, "path", rollout)
	return strings.TrimSpace(string(out)), err
}

func turnsIn(m sh.Machine, transcript string) (int, error) {
	out, err := sh.Run(m, sessionScript, "turns", transcript)
	if err != nil {
		return 0, err
	}
	n, err := strconv.Atoi(strings.TrimSpace(string(out)))
	if err != nil {
		return 0, fmt.Errorf("unexpected turn count %q from %s", out, m.Name())
	}
	return n, nil
}

func retireCopy(from, to sh.Machine, toName, id, transcript string, ws Workspace, exclude []string) []error {
	return []error{settle(from, ws, exclude), retireTranscript(from, transcript, toName), markMoved(from, id, toName), clearMoved(to, id)}
}

type sessionCopy struct {
	m          sh.Machine
	here       bool
	name       string
	dir        string
	transcript string
	procs      []string
	used       bool
}

func (c *sessionCopy) place() string {
	if c.here {
		return "here"
	}
	return "on " + c.name
}

func (c *sessionCopy) running() bool {
	return len(c.procs) > 0
}

func (c *sessionCopy) look(j *moveJournal) error {
	if j.Codex {
		running, _, err := codexRunning(c.m, j.Session, c.transcript, c.dir)
		if err != nil && sh.ExitCode(err) != sh.ExitGone {
			return err
		}
		for _, pid := range running {
			c.procs = append(c.procs, strconv.Itoa(pid), "")
		}
	} else {
		sessions, err := liveSessions(c.m)
		if err != nil {
			return err
		}
		for _, s := range sessions {
			if s.SessionID == j.Session {
				c.procs = append(c.procs, strconv.Itoa(s.PID), s.since())
			}
		}
	}
	turns, err := turnsIn(c.m, c.transcript)
	c.used = turns > j.Turns
	return err
}

func (c *sessionCopy) stop() error {
	if !c.running() {
		return nil
	}
	_, err := sh.Run(c.m, sessionScript, append([]string{"end"}, c.procs...)...)
	return err
}

type recovery struct {
	j           *moveJournal
	host        Host
	remote      sh.Machine
	here, there *sessionCopy
	from, to    *sessionCopy
}

func (j *moveJournal) recovery(local, remote sh.Machine, host Host) *recovery {
	r := &recovery{j: j, host: host, remote: remote,
		here:  &sessionCopy{m: local, here: true, name: local.Name(), transcript: j.LocalTranscript},
		there: &sessionCopy{m: remote, name: j.Host, transcript: j.RemoteTranscript},
	}
	r.from, r.to = r.here, r.there
	if j.Kind == "pull" {
		r.from, r.to = r.there, r.here
	}
	r.from.dir, r.to.dir = filepath.Join(j.Workspace.Root, j.Workspace.Subdir), filepath.Join(j.Where.Root, j.Workspace.Subdir)
	return r
}

func (r *recovery) giveBack(err error) error {
	if errors.Is(err, errFolderAppeared) {
		r.j.Where.NewFolder = ""
	}
	note, undoErr := r.undo()
	if undoErr == nil {
		undoErr = forgetMove(r.j.Session)
	}
	switch {
	case undoErr != nil:
		return fmt.Errorf("%w; undoing it also failed: %v", err, undoErr)
	case note != "":
		return fmt.Errorf("%w (%s)", err, note)
	}
	return err
}

func (r *recovery) live(reached bool) *sessionCopy {
	from, to := r.from, r.to
	switch {
	case r.j.Step == "stop":
		return from
	case from.used && to.used && from.running() == to.running():
		return nil
	case from.used && to.used && to.running(), !from.used && to.used:
		return to
	case from.used, r.j.Undone || !r.j.reached("retire"):
		return from
	case !reached && r.here.running():
		return r.here
	}
	return to
}

type recovered struct {
	report  string
	here    bool
	settled bool
}

func recoverMove(id string) recovered {
	path, err := movePath(id)
	if err != nil {
		return recovered{report: err.Error() + "."}
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return recovered{settled: true}
	}
	j := &moveJournal{Session: id}
	if err == nil {
		err = json.Unmarshal(data, j)
	}
	if err != nil {
		return recovered{report: fmt.Sprintf("The record of an interrupted move of session %s can't be read (%v); remove %s.", store.ShortID(id), err, path)}
	}
	if j.Retirer > 0 {
		if args, _ := exec.Command("ps", "-ww", "-o", "args=", "-p", strconv.Itoa(j.Retirer)).Output(); strings.Contains(string(args), path) {
			return recovered{report: fmt.Sprintf("Session %s is still being closed here after its move to %s. Run magi status again in a minute.", store.ShortID(id), j.Host)}
		}
	}
	what := fmt.Sprintf("Moving session %s to %s was interrupted", store.ShortID(id), j.Host)
	if j.Kind == "pull" {
		what = fmt.Sprintf("Bringing session %s back from %s was interrupted", store.ShortID(id), j.Host)
	}
	failed := func(err error) recovered {
		return recovered{report: fmt.Sprintf("%s, and checking it failed: %v. Run magi status again.", what, err)}
	}
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return failed(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), recoverBudget)
	defer cancel()
	host, reach := loadHost(j.Host)
	var remote *sh.Remote
	if reach == nil {
		if remote, err = newRemote(ctx, host); err != nil {
			return failed(err)
		}
	}
	r := j.recovery(local, remote, host)
	if err := r.here.look(j); err != nil {
		return failed(err)
	}
	switch {
	case errors.Is(reach, errNoHost):
	case reach != nil:
		return failed(reach)
	default:
		if reach = r.there.look(j); reach != nil && sh.ExitCode(reach) != sh.ExitUnreachable && ctx.Err() == nil {
			return failed(reach)
		}
		if reach != nil {
			r.there.procs = nil
		}
	}
	live := r.live(reach == nil)
	switch {
	case live == nil:
		advice := "magi kept both. Continue the one you want"
		if r.here.running() {
			advice = "magi stopped neither. Quit the one you don't want"
		}
		return recovered{report: fmt.Sprintf("%s, and both copies have new turns since, so %s, then run magi status again. Here: %s. On %s: %s.", what, advice, r.next(r.here), j.Host, r.next(r.there))}
	case reach != nil && !live.here:
		return recovered{report: fmt.Sprintf("%s, and magi can't reach %s (%v). The session is still on %s. Run magi status again once it answers; if it's gone for good, remove %s.", what, j.Host, reach, j.Host, path)}
	case reach != nil:
		if err := clearMoved(local, id); err != nil {
			return failed(err)
		}
		return recovered{here: true, report: fmt.Sprintf("%s, and magi can't reach %s (%v). The copy here is the live one; continue it here: %s. Run magi status again once %s answers to finish cleaning up there; if it's gone for good, remove %s.", what, j.Host, reach, r.next(r.here), j.Host, path)}
	}
	other, done := r.from, "closed the copy "+r.from.place()
	if live == r.to {
		err = r.finish()
	} else {
		var note string
		other, done = r.to, "set aside the copy "+r.to.place()
		if note, err = r.undo(); note != "" {
			done += " (" + note + ")"
		}
	}
	if err == nil {
		err = forgetMove(id)
	}
	if err != nil {
		return recovered{report: fmt.Sprintf("%s. The copy %s is the live one, but cleaning up %s failed: %v. Run magi status again.", what, live.place(), other.place(), err)}
	}
	if other.used {
		done += fmt.Sprintf(". The turns taken %s are kept in %s.handed-off-to-%s", other.place(), other.transcript, live.name)
	}
	return recovered{here: live.here, settled: true, report: fmt.Sprintf("%s. The copy %s is the live one, so magi %s. Continue it %s: %s.", what, live.place(), done, live.place(), r.next(live))}
}

func (r *recovery) finish() error {
	j, from, to := r.j, r.from, r.to
	if err := from.stop(); err != nil {
		return err
	}
	errs := retireCopy(from.m, to.m, to.name, j.Session, from.transcript, j.Workspace, r.host.Exclude)
	if j.Kind == "push" {
		errs = append(errs, closeForwardsOn(r.remote, j.Session))
		if j.Tab != nil {
			errs = append(errs, recordTab(r.remote, j.Session, *j.Tab))
		}
	} else {
		errs = append(errs, closeRecordedTab(r.remote, j.Session), closeForwards(j.Session))
	}
	entries, err := readLog()
	if err == nil && !slices.ContainsFunc(entries, func(e logEntry) bool { return e.Session == j.Session && e.Kind == j.Kind && !e.Time.Before(j.Started) }) {
		err = appendLog(logEntry{Time: time.Now(), Kind: j.Kind, Session: j.Session, From: from.name, To: to.name, FromDir: from.dir, ToDir: to.dir})
	}
	return errors.Join(append(errs, err)...)
}

func (r *recovery) undo() (string, error) {
	j, from, to := r.j, r.from, r.to
	j.Undone = true
	if err := j.at(j.Step); err != nil {
		return "", err
	}
	if err := to.stop(); err != nil {
		return "", err
	}
	if to.procs == nil && j.Kind == "push" {
		if err := stopSession(r.remote, j.Session, j.Codex); err != nil {
			return "", err
		}
	}
	if j.Tab != nil {
		if err := (sh.Tmux{Machine: r.remote}).Discard(*j.Tab); err != nil && !sh.TabGone(err) {
			return "", err
		}
	}
	errs := []error{retireTranscript(to.m, to.transcript, from.name)}
	if j.Kind == "push" {
		errs = append(errs, closeForwards(j.Session))
	}
	if j.reached("finish", "retire") {
		errs = append(errs, clearMoved(from.m, j.Session))
		if j.Workspace.Git != nil {
			errs = append(errs, unparkRepo(from.m, j.Workspace.Root))
		}
	}
	var note string
	switch synced := !j.reached("stop", "sync"); {
	case synced && j.Workspace.Git != nil:
		note = fmt.Sprintf("%s on %s has this session's work now; trying again updates it", j.Where.Root, to.m.Name())
		if err := park(to.m, j.Where.Root, j.Workspace); err != nil {
			errs = append(errs, fmt.Errorf("%s on %s has this session's work now, and marking it as handed off failed: %v", j.Where.Root, to.m.Name(), err))
		}
	case synced:
		note = fmt.Sprintf("%s on %s has this session's files now; trying again updates it", j.Where.Root, to.m.Name())
	case j.Where.NewFolder != "":
		note = fmt.Sprintf("the folder %s it created on %s is still there; remove it if you don't need it", j.Where.NewFolder, to.m.Name())
	}
	return note, errors.Join(errs...)
}

func (r *recovery) next(c *sessionCopy) string {
	switch {
	case !c.running():
		return fmt.Sprintf("run %s in %s", resumeHint(remoteSession{ID: r.j.Session, Codex: r.j.Codex}), c.dir)
	case c.here || r.j.Tab == nil:
		return "it's still open, in " + c.dir
	case r.j.Codex:
		return "it's open " + sh.Tmux{Machine: r.remote}.Where(r.j.Where.Group)
	}
	return "it's open " + sh.Tmux{Machine: r.remote}.Where(r.j.Where.Group) + ", and in the Claude apps through Remote Control"
}

func recoverBefore(id string, here bool) error {
	rec := recoverMove(id)
	if rec.report == "" || rec.settled && rec.here == here {
		return nil
	}
	return errors.New(strings.TrimSuffix(rec.report, "."))
}

func reportMoves(w io.Writer) error {
	dir, err := store.Dir("moves")
	if err != nil {
		return err
	}
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil {
		return err
	}
	var reports []string
	for _, path := range paths {
		id := strings.TrimSuffix(filepath.Base(path), ".json")
		unlock, err := lockMove(id)
		if err != nil {
			continue
		}
		reports = append(reports, recoverMove(id).report)
		unlock()
	}
	if len(reports) == 0 {
		return nil
	}
	fmt.Fprintln(w, "Interrupted moves:")
	for _, report := range reports {
		fmt.Fprintf(w, "  %s\n", report)
	}
	fmt.Fprintln(w)
	return nil
}
