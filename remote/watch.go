package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"math"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const (
	watcherFresh  = 15 * time.Second
	historyKept   = time.Hour
	watchRetry    = time.Minute
	watchBackoff  = time.Minute
	tailscaleWait = 5 * time.Second
)

type wanted struct {
	To   string `json:"to"`
	PID  int    `json:"pid"`
	Note string `json:"note"`
}

func wantedNote(m sh.Machine, sessionID string) (string, error) {
	out, err := sh.Run(m, `f="$HOME/.magi/wanted/$1.json"; [ ! -f "$f" ] || cat "$f"`, sessionID)
	if err != nil || len(out) == 0 {
		return "", err
	}
	var w wanted
	if err := json.Unmarshal(out, &w); err != nil {
		return "", fmt.Errorf("the request to take session %s back is unreadable on %s: %w", store.ShortID(sessionID), m.Name(), err)
	}
	return w.Note, nil
}

func writeWanted(sessionID string, w wanted) error {
	path, err := store.SessionPath("wanted", sessionID, ".json")
	if err != nil {
		return err
	}
	return store.WriteJSON(path, w)
}

type copies struct {
	Secrets []string `json:"secrets"`
	Exclude []string `json:"exclude"`
}

func takerCopies(name string) (Host, error) {
	path, err := store.Dir(filepath.Join("watchers", name))
	if err != nil {
		return Host{}, err
	}
	var c copies
	if err := store.ReadJSON(path, &c); err != nil {
		return Host{}, fmt.Errorf("%s doesn't say which files it copies when it takes a session back (%v); update magi there", name, err)
	}
	return Host{Secrets: c.Secrets, Exclude: c.Exclude}, nil
}

func watchers() ([]string, error) {
	dir, err := store.Dir("watchers")
	if err != nil {
		return nil, err
	}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var fresh []string
	for _, e := range entries {
		info, err := e.Info()
		if err == nil && time.Since(info.ModTime()) < watcherFresh {
			fresh = append(fresh, e.Name())
		}
	}
	return fresh, nil
}

func waitForPull(input stopInput, out io.Writer) error {
	path, err := store.SessionPath("wanted", input.SessionID, ".json")
	if err != nil {
		return err
	}
	var w wanted
	err = store.ReadJSON(path, &w)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	w.Note = stillRunning(w.Note, input.BackgroundTasks)
	if err := writeWanted(input.SessionID, w); err != nil {
		return err
	}
	return json.NewEncoder(out).Encode(map[string]string{
		"systemMessage": fmt.Sprintf("Waiting for %s to take this session back.", w.To),
	})
}

func reportWanted(w io.Writer) error {
	dir, err := store.Dir("wanted")
	if err != nil {
		return err
	}
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil || len(paths) == 0 {
		return err
	}
	fmt.Fprintln(w, "Waiting to be taken back:")
	for _, path := range paths {
		var request wanted
		if err := store.ReadJSON(path, &request); err != nil {
			return err
		}
		fmt.Fprintf(w, "  %s  by %s\n", store.ShortID(strings.TrimSuffix(filepath.Base(path), ".json")), request.To)
	}
	fmt.Fprintln(w)
	return nil
}

func watch() error {
	path, err := store.Dir("watch.lock")
	if err != nil {
		return err
	}
	unlock, err := store.Lock(path, errors.New("magi watch is already running on this machine"))
	if err != nil {
		return err
	}
	defer unlock()
	cfg, _, err := loadConfig()
	if err != nil {
		return err
	}
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return err
	}
	logger := log.New(os.Stderr, "", log.LstdFlags)
	quit := func() { stopWatching(append([]string{local.Name()}, cfg.Order...), logger) }
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM, syscall.SIGINT)
	go func() {
		<-signals
		quit()
	}()
	go heartbeat(logger, quit)
	var wg sync.WaitGroup
	wg.Go(func() {
		watchHost(newWatcher(local.Name(), "", nil, logger, ""), "", func() (sh.Machine, error) { return local, nil })
	})
	for _, name := range cfg.Order {
		host := cfg.host(name)
		said, err := json.Marshal(copies{Secrets: host.Secrets, Exclude: host.Exclude})
		if err != nil {
			return err
		}
		wg.Go(func() {
			watchHost(newWatcher(name, host.SSH, nil, logger, string(said)), local.Name(), func() (sh.Machine, error) {
				return newRemote(context.Background(), host)
			})
		})
	}
	wg.Wait()
	return nil
}

var heartbeatEvery = time.Minute

var taking sync.RWMutex

func heartbeat(logger *log.Logger, restart func()) {
	link, err := magiLink()
	if err != nil {
		logger.Print(err)
		return
	}
	path, err := configPath()
	if err != nil {
		logger.Print(err)
		return
	}
	self, _ := filepath.EvalSymlinks(link)
	config, _ := os.ReadFile(path)
	for range time.Tick(heartbeatEvery) {
		moves, err := sweepMoves()
		if err != nil {
			logger.Printf("checking interrupted moves failed: %v", err)
		}
		for _, r := range moves {
			if r.settled {
				logger.Print(r.report)
			}
		}
		changed := ""
		if now, err := filepath.EvalSymlinks(link); err == nil && now != self {
			changed = "magi was updated to " + now
		}
		if now, _ := os.ReadFile(path); !bytes.Equal(now, config) {
			changed = "magi's machines changed"
		}
		if changed != "" {
			logger.Printf("%s; restarting once no session is being taken back", changed)
			taking.Lock()
			restart()
		}
	}
}

func watchHost(w *watcher, me string, connect func() (sh.Machine, error)) {
	wait := time.Second
	for {
		started := time.Now()
		m, err := connect()
		if err == nil {
			w.m = m
			err = w.follow(me)
		}
		if time.Since(started) > watchBackoff {
			wait = time.Second
		}
		w.logger.Printf("%s: watching stopped (%v); trying again in %s", w.name, err, wait)
		time.Sleep(wait)
		wait = min(wait*2, watchBackoff)
	}
}

type watcher struct {
	name   string
	m      sh.Machine
	copies string
	logger *log.Logger
	live   liveMachine
	block  []string
	usage  map[int]use
	cpu    map[string][]cpuReading
}

type liveMachine struct {
	Answering bool        `json:"answering"`
	Heard     time.Time   `json:"heard,omitzero"`
	SSH       string      `json:"ssh,omitempty"`
	Cores     int         `json:"cores"`
	Load      float64     `json:"load"`
	Memory    space       `json:"memory"`
	Disk      space       `json:"disk"`
	Pressure  string      `json:"pressure,omitempty"`
	Swap      space       `json:"swap,omitzero"`
	BootedAt  time.Time   `json:"bootedAt,omitzero"`
	Battery   *battery    `json:"battery,omitempty"`
	Relayed   bool        `json:"relayed,omitempty"`
	History   []reading   `json:"history,omitempty"`
	Sessions  []liveEntry `json:"sessions"`
}

type reading struct {
	At       int64   `json:"at"`
	Load     float64 `json:"load"`
	MemoryMB int     `json:"memoryMB"`
}

type cpuReading struct {
	At  int64 `json:"at"`
	CPU int   `json:"cpu"`
}

type space struct {
	UsedMB  int `json:"usedMB"`
	TotalMB int `json:"totalMB"`
}

type battery struct {
	Percent   int  `json:"percent"`
	Charging  bool `json:"charging"`
	OnBattery bool `json:"onBattery"`
}

type liveEntry struct {
	ID            string       `json:"id"`
	Title         string       `json:"title"`
	Folder        string       `json:"folder"`
	State         string       `json:"state"`
	WaitingOn     string       `json:"waitingOn,omitempty"`
	Since         time.Time    `json:"since,omitzero"`
	RemoteControl string       `json:"remoteControl,omitempty"`
	Tmux          string       `json:"tmux,omitempty"`
	CPU           int          `json:"cpu"`
	MemoryMB      int          `json:"memoryMB"`
	CPUHistory    []cpuReading `json:"cpuHistory,omitempty"`
	pid           int
}

type use struct {
	seconds       float64
	at            time.Time
	cpu, memoryMB int
}

func newWatcher(name, ssh string, m sh.Machine, logger *log.Logger, copies string) *watcher {
	w := &watcher{name: name, m: m, copies: copies, logger: logger, live: liveMachine{SSH: ssh, Sessions: []liveEntry{}}, cpu: map[string][]cpuReading{}}
	var kept liveMachine
	if path, err := livePath(name); err == nil && store.ReadJSON(path, &kept) == nil {
		w.live.History = kept.History
		for _, s := range kept.Sessions {
			w.cpu[s.ID] = s.CPUHistory
		}
	}
	return w
}

func recent[T any](readings []T, at func(T) int64, now time.Time) []T {
	from := now.Add(-historyKept).Unix()
	for len(readings) > 0 && at(readings[0]) < from {
		readings = readings[1:]
	}
	return readings
}

func (w *watcher) follow(me string) error {
	w.block = nil
	cmd := w.m.Command(watchScript, me, w.m.Claude(), w.copies)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	tried := map[string]time.Time{}
	lines := bufio.NewScanner(stdout)
	lines.Buffer(nil, math.MaxInt)
	for lines.Scan() {
		line := lines.Text()
		if !store.SessionIDPattern.MatchString(line) {
			w.hear(line, time.Now())
			continue
		}
		if time.Since(tried[line]) < watchRetry {
			continue
		}
		tried[line] = time.Now()
		go take(w.name, w.m, line, w.logger)
	}
	err = cmd.Wait()
	w.live.Answering = false
	return errors.Join(err, w.save())
}

func (w *watcher) hear(line string, now time.Time) {
	kind, rest, _ := strings.Cut(line, " ")
	switch kind {
	case "claude", "waiting", "tab":
		w.block = append(w.block, line)
		return
	case "sessions":
		w.sessions()
	case "stats":
		if err := w.stats(rest, now); err != nil {
			w.logger.Printf("%s: unreadable stats %q: %v", w.name, rest, err)
			return
		}
	}
	w.live.Answering, w.live.Heard = true, now
	if err := w.save(); err != nil {
		w.logger.Printf("%s: writing what it runs failed: %v", w.name, err)
	}
}

func (w *watcher) sessions() {
	var sessions []liveSession
	waiting := map[int]string{}
	windows := map[int]string{}
	for _, line := range w.block {
		kind, rest, _ := strings.Cut(line, " ")
		switch kind {
		case "claude":
			var s liveSession
			if json.Unmarshal([]byte(rest), &s) == nil {
				sessions = append(sessions, s)
			}
		case "waiting":
			var listed []liveSession
			json.Unmarshal([]byte(rest), &listed)
			for _, s := range listed {
				waiting[s.PID] = s.WaitingFor
			}
		case "tab":
			pid, window, _ := strings.Cut(rest, " ")
			n, _ := strconv.Atoi(pid)
			windows[n] = window
		}
	}
	w.block = nil
	entries := []liveEntry{}
	for _, s := range sessions {
		e := liveEntry{ID: s.SessionID, Title: sessionTitle(s), Folder: s.Cwd, State: s.Status, WaitingOn: waiting[s.PID], RemoteControl: s.remoteControl(), Tmux: windows[s.PID], pid: s.PID}
		if s.StatusAt > 0 {
			e.Since = time.UnixMilli(s.StatusAt)
		}
		entries = append(entries, e)
	}
	w.live.Sessions = entries
}

func (w *watcher) stats(line string, now time.Time) error {
	l := &w.live
	var booted int64
	if _, err := fmt.Sscan(line, &l.Cores, &l.Load, &l.Memory.UsedMB, &l.Memory.TotalMB, &l.Disk.UsedMB, &l.Disk.TotalMB, &l.Swap.UsedMB, &l.Swap.TotalMB, &booted); err != nil {
		return err
	}
	l.BootedAt, l.Pressure, l.Battery = time.Unix(booted, 0), "", nil
	l.Relayed = l.SSH != "" && relayed(l.SSH)
	l.History = recent(append(l.History, reading{At: now.Unix(), Load: l.Load, MemoryMB: l.Memory.UsedMB}), func(r reading) int64 { return r.At }, now)
	ids := map[int]string{}
	cpu := map[string][]cpuReading{}
	for _, s := range l.Sessions {
		ids[s.pid] = s.ID
		cpu[s.ID] = w.cpu[s.ID]
	}
	usage := map[int]use{}
	for _, field := range strings.Fields(line)[9:] {
		if pressure, ok := strings.CutPrefix(field, "pressure="); ok {
			l.Pressure = pressure
			continue
		}
		if charge, ok := strings.CutPrefix(field, "battery="); ok {
			l.Battery = &battery{}
			if _, err := fmt.Sscanf(charge, "%d:%t:%t", &l.Battery.Percent, &l.Battery.Charging, &l.Battery.OnBattery); err != nil {
				return err
			}
			continue
		}
		var pid int
		u := use{at: now}
		if _, err := fmt.Sscanf(field, "%d:%g:%d", &pid, &u.seconds, &u.memoryMB); err != nil {
			return err
		}
		if last, ok := w.usage[pid]; ok {
			u.cpu = int(math.Round(max(0, (u.seconds-last.seconds)/now.Sub(last.at).Seconds()*100)))
			if id, ok := ids[pid]; ok {
				cpu[id] = append(cpu[id], cpuReading{At: now.Unix(), CPU: u.cpu})
			}
		}
		usage[pid] = u
	}
	for id, readings := range cpu {
		cpu[id] = recent(readings, func(r cpuReading) int64 { return r.At }, now)
	}
	w.usage, w.cpu = usage, cpu
	return nil
}

func relayed(ssh string) bool {
	var status struct {
		Peer map[string]struct{ HostName, DNSName, CurAddr string }
	}
	ctx, cancel := context.WithTimeout(context.Background(), tailscaleWait)
	defer cancel()
	if out, err := exec.CommandContext(ctx, "tailscale", "status", "--json").Output(); err == nil {
		json.Unmarshal(out, &status)
	}
	host := ssh[strings.LastIndex(ssh, "@")+1:]
	for _, p := range status.Peer {
		if p.HostName == host || strings.HasPrefix(p.DNSName, host+".") {
			return p.CurAddr == ""
		}
	}
	return false
}

func (w *watcher) save() error {
	for i := range w.live.Sessions {
		s := &w.live.Sessions[i]
		s.CPU, s.MemoryMB, s.CPUHistory = w.usage[s.pid].cpu, w.usage[s.pid].memoryMB, w.cpu[s.ID]
	}
	path, err := livePath(w.name)
	if err != nil {
		return err
	}
	liveFiles.Lock()
	defer liveFiles.Unlock()
	return store.WriteJSON(path, w.live)
}

var liveFiles sync.Mutex

func livePath(name string) (string, error) {
	return store.Dir(filepath.Join("live", name+".json"))
}

func stopWatching(names []string, logger *log.Logger) {
	liveFiles.Lock()
	for _, name := range names {
		var live liveMachine
		path, err := livePath(name)
		if err == nil {
			err = store.ReadJSON(path, &live)
		}
		if err == nil {
			live.Answering = false
			err = store.WriteJSON(path, live)
		}
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			logger.Printf("%s: marking it as not watched failed: %v", name, err)
		}
	}
	os.Exit(0)
}

func take(name string, remote sh.Machine, id string, logger *log.Logger) {
	taking.RLock()
	defer taking.RUnlock()
	logger.Printf("%s: taking session %s back", name, store.ShortID(id))
	message, err := pullHere(name, id, logger.Writer(), true)
	if err != nil {
		logger.Printf("%s: taking session %s back failed: %v", name, store.ShortID(id), err)
		return
	}
	if _, err := sh.Run(remote, `rm -f "$HOME/.magi/wanted/$1.json"`, id); err != nil {
		logger.Printf("%s: removing the request for session %s failed: %v", name, store.ShortID(id), err)
	}
	logger.Print(message)
}
