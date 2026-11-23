package main

import (
	"bufio"
	"bytes"
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"math"
	"net/url"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
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
	tailnetEvery  = 30 * time.Second
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
	go followTailnet(logger)
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
	var appTried time.Time
	var appCurrent, appUpdating atomic.Bool
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
		} else if !appCurrent.Load() && !appUpdating.Load() && time.Since(appTried) >= time.Hour {
			appTried = time.Now()
			appUpdating.Store(true)
			go func() {
				appCurrent.Store(updateApp(logger))
				appUpdating.Store(false)
			}()
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
	name     string
	m        sh.Machine
	copies   string
	logger   *log.Logger
	live     liveMachine
	block    []string
	measured bool
	usage    map[int]use
	cpu      map[string][]cpuReading
	ticks    ticks
	traffic  traffic
	nics     map[string]traffic
	sampled  time.Time
}

type ticks struct {
	busy, total int64
}

type traffic struct {
	iface          string
	received, sent int64
	at             time.Time
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
	Critical  *float64    `json:"critical,omitempty"`
	Relayed   bool        `json:"relayed,omitempty"`
	History   []reading   `json:"history,omitempty"`
	Sessions  []liveEntry `json:"sessions"`

	Top         *top         `json:"top,omitempty"`
	MemoryParts []memoryPart `json:"memoryParts,omitempty"`
	Volumes     []volume     `json:"volumes,omitempty"`
	Interfaces  []nic        `json:"interfaces,omitempty"`
	Sensors     []sensor     `json:"sensors,omitempty"`
}

type top struct {
	CPU    []busy  `json:"cpu"`
	Memory []large `json:"memory"`
}

type busy struct {
	PID     int    `json:"pid"`
	Name    string `json:"name"`
	CPU     int    `json:"cpu"`
	Session string `json:"session,omitempty"`
}

type large struct {
	PID      int    `json:"pid"`
	Name     string `json:"name"`
	MemoryMB int    `json:"memoryMB"`
	Session  string `json:"session,omitempty"`
}

type process struct {
	pid, memoryMB, root int
	name                string
	seconds             float64
}

type memoryPart struct {
	Kind string `json:"kind"`
	MB   int    `json:"mb"`
}

type volume struct {
	Name    string `json:"name"`
	Mount   string `json:"mount"`
	UsedMB  int    `json:"usedMB"`
	TotalMB int    `json:"totalMB"`
}

type nic struct {
	Name     string `json:"name"`
	Received int64  `json:"rx"`
	Sent     int64  `json:"tx"`
}

type sensor struct {
	Name     string   `json:"name"`
	Temp     float64  `json:"temp"`
	Critical *float64 `json:"critical,omitempty"`
}

type details struct {
	procs []process
	nics  map[string]traffic
}

type reading struct {
	At       int64    `json:"at"`
	CPU      *int     `json:"cpu,omitempty"`
	Temp     *float64 `json:"temp,omitempty"`
	Thermal  *int     `json:"thermal,omitempty"`
	Received *int64   `json:"rx,omitempty"`
	Sent     *int64   `json:"tx,omitempty"`
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
	OnBattery bool `json:"onBattery"`
}

type liveEntry struct {
	ID            string    `json:"id"`
	Title         string    `json:"title"`
	Folder        string    `json:"folder"`
	State         string    `json:"state"`
	WaitingOn     string    `json:"waitingOn,omitempty"`
	Since         time.Time `json:"since,omitzero"`
	RemoteControl string    `json:"remoteControl,omitempty"`
	HostSession   string    `json:"hostSession,omitempty"`
	location
	CPU        int          `json:"cpu"`
	MemoryMB   int          `json:"memoryMB"`
	CPUHistory []cpuReading `json:"cpuHistory,omitempty"`
	pid        int
}

type location struct {
	Tmux string `json:"tmux,omitempty"`
	Rex  *rex   `json:"rex,omitempty"`
	App  string `json:"app,omitempty"`
	TTY  string `json:"tty,omitempty"`
}

type rex struct {
	Session string `json:"session"`
	Block   string `json:"block"`
}

type use struct {
	seconds       float64
	at            time.Time
	cpu, memoryMB int
}

func newWatcher(name, ssh string, m sh.Machine, logger *log.Logger, copies string) *watcher {
	w := &watcher{name: name, m: m, copies: copies, logger: logger, cpu: map[string][]cpuReading{}}
	if path, err := livePath(name); err == nil {
		store.ReadJSON(path, &w.live)
	}
	for _, s := range w.live.Sessions {
		w.cpu[s.ID] = s.CPUHistory
	}
	w.live.SSH, w.live.Answering, w.live.Sessions = ssh, false, []liveEntry{}
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
	w.block, w.measured = nil, false
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
	case "claude", "waiting", "place":
		w.block = append(w.block, line)
		return
	case "sessions":
		w.sessions()
	case "stats":
		err := w.stats(rest, now)
		w.sampled = now
		if err != nil {
			w.logger.Printf("%s: unreadable stats %q: %v", w.name, rest, err)
			return
		}
		w.measured = true
	}
	if !w.measured {
		return
	}
	w.live.Answering, w.live.Heard = true, now
	if err := w.save(); err != nil {
		w.logger.Printf("%s: writing what it runs failed: %v", w.name, err)
	}
}

func (w *watcher) sessions() {
	var sessions []liveSession
	waiting := map[int]string{}
	places := map[int]location{}
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
		case "place":
			pid, rest, _ := strings.Cut(rest, " ")
			key, value, _ := strings.Cut(rest, " ")
			n, _ := strconv.Atoi(pid)
			p := places[n]
			switch key {
			case "tmux":
				p.Tmux = value
			case "rex":
				session, block, _ := strings.Cut(value, " ")
				p.Rex = &rex{Session: session, Block: block}
			case "app":
				p.App = value
			case "tty":
				p.TTY = value
			}
			places[n] = p
		}
	}
	w.block = nil
	entries := []liveEntry{}
	at := map[string]int{}
	for _, s := range sessions {
		e := liveEntry{ID: s.SessionID, Title: sessionTitle(s), Folder: s.Cwd, State: s.Status, WaitingOn: waiting[s.PID], RemoteControl: s.remoteControl(), location: places[s.PID], pid: s.PID}
		if s.Entrypoint == "claude-desktop" {
			e.HostSession = s.HostSessionID
		}
		if s.StatusAt > 0 {
			e.Since = time.UnixMilli(s.StatusAt)
		}
		if i, ok := at[e.ID]; ok {
			if e.Since.After(entries[i].Since) {
				entries[i] = e
			}
			continue
		}
		at[e.ID] = len(entries)
		entries = append(entries, e)
	}
	w.live.Sessions = entries
}

func (w *watcher) stats(line string, now time.Time) error {
	l, t, n := w.live, w.ticks, w.traffic
	var booted int64
	if _, err := fmt.Sscan(line, &l.Cores, &l.Load, &l.Memory.UsedMB, &l.Memory.TotalMB, &l.Disk.UsedMB, &l.Disk.TotalMB, &l.Swap.UsedMB, &l.Swap.TotalMB, &booted); err != nil {
		return err
	}
	l.BootedAt, l.Pressure, l.Battery, l.Critical = time.Unix(booted, 0), "", nil, nil
	l.Top, l.MemoryParts, l.Volumes, l.Interfaces, l.Sensors = nil, nil, nil, nil, nil
	d := details{nics: map[string]traffic{}}
	l.Relayed = relayed(l.SSH)
	r := reading{At: now.Unix()}
	ids := map[int]string{}
	cpu := map[string][]cpuReading{}
	for _, s := range l.Sessions {
		ids[s.pid] = s.ID
		cpu[s.ID] = w.cpu[s.ID]
	}
	usage := map[int]use{}
	for _, field := range strings.FieldsFunc(line, func(r rune) bool { return r == ' ' })[9:] {
		if key, value, ok := strings.Cut(field, "="); ok {
			if err := measure(key, value, now, &l, &r, &t, &n, &d); err != nil {
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
	l.History = recent(append(l.History, r), func(r reading) int64 { return r.At }, now)
	if len(d.procs) > 0 {
		l.Top = &top{}
		for _, p := range largest(d.procs, func(p process) float64 { return p.seconds }) {
			l.Top.CPU = append(l.Top.CPU, busy{PID: p.pid, Name: p.name, CPU: int(math.Round(p.seconds / now.Sub(w.sampled).Seconds() * 100)), Session: ids[p.root]})
		}
		for _, p := range largest(d.procs, func(p process) float64 { return float64(p.memoryMB) }) {
			l.Top.Memory = append(l.Top.Memory, large{PID: p.pid, Name: p.name, MemoryMB: p.memoryMB, Session: ids[p.root]})
		}
	}
	for name, t := range d.nics {
		if received, sent, ok := t.since(w.nics[name]); ok && received+sent > 0 {
			l.Interfaces = append(l.Interfaces, nic{Name: name, Received: received, Sent: sent})
		}
	}
	l.Interfaces = largest(l.Interfaces, func(n nic) float64 { return float64(n.Received + n.Sent) })
	l.Volumes = largest(l.Volumes, func(v volume) float64 { return float64(v.TotalMB) })
	l.Sensors = largest(l.Sensors, func(s sensor) float64 { return s.Temp })
	w.live, w.ticks, w.traffic, w.usage, w.cpu, w.nics = l, t, n, usage, cpu, d.nics
	return nil
}

func largest[T any](items []T, size func(T) float64) []T {
	items = slices.Clone(items)
	slices.SortStableFunc(items, func(a, b T) int { return cmp.Compare(size(b), size(a)) })
	return items[:min(len(items), 8)]
}

func (t traffic) since(was traffic) (received, sent int64, ok bool) {
	if t.iface != was.iface || t.received < was.received || t.sent < was.sent {
		return 0, 0, false
	}
	seconds := t.at.Sub(was.at).Seconds()
	return int64(math.Round(float64(t.received-was.received) / seconds)), int64(math.Round(float64(t.sent-was.sent) / seconds)), true
}

func measure(key, value string, now time.Time, l *liveMachine, r *reading, cpu *ticks, net *traffic, d *details) error {
	switch key {
	case "pressure":
		l.Pressure = value
	case "battery":
		l.Battery = &battery{}
		_, err := fmt.Sscanf(value, "%d:%t", &l.Battery.Percent, &l.Battery.OnBattery)
		return err
	case "cpu":
		r.CPU = new(int)
		_, err := fmt.Sscan(value, r.CPU)
		return err
	case "ticks":
		var t ticks
		if _, err := fmt.Sscanf(value, "%d:%d", &t.busy, &t.total); err != nil {
			return err
		}
		if cpu.total > 0 && t.total > cpu.total {
			r.CPU = new(int(math.Round(float64(t.busy-cpu.busy) / float64(t.total-cpu.total) * 100)))
		}
		*cpu = t
	case "temp":
		r.Temp = new(float64)
		var critical float64
		n, err := fmt.Sscanf(value, "%g:%g", r.Temp, &critical)
		if n == 0 {
			return err
		}
		if n == 2 {
			l.Critical = &critical
		}
	case "thermal":
		r.Thermal = new(int)
		_, err := fmt.Sscan(value, r.Thermal)
		return err
	case "net":
		t := traffic{at: now}
		if _, err := fmt.Sscanf(value, "%d:%d:%s", &t.received, &t.sent, &t.iface); err != nil {
			return err
		}
		if received, sent, ok := t.since(*net); ok {
			r.Received, r.Sent = &received, &sent
		}
		*net = t
	case "nic":
		t := traffic{at: now}
		if _, err := fmt.Sscanf(value, "%d:%d:%s", &t.received, &t.sent, &t.iface); err != nil {
			return err
		}
		d.nics[t.iface] = t
	case "proc":
		f := strings.SplitN(value, ":", 5)
		if len(f) != 5 {
			return fmt.Errorf("process %q lacks a field", value)
		}
		p := process{name: f[4]}
		if _, err := fmt.Sscanf(value, "%d:%g:%d:%d:", &p.pid, &p.seconds, &p.memoryMB, &p.root); err != nil {
			return err
		}
		if err := unescape(&p.name); err != nil {
			return err
		}
		d.procs = append(d.procs, p)
	case "part":
		kind, mb, _ := strings.Cut(value, ":")
		n, err := strconv.Atoi(mb)
		l.MemoryParts = append(l.MemoryParts, memoryPart{Kind: kind, MB: n})
		return err
	case "vol":
		f := strings.SplitN(value, ":", 4)
		if len(f) != 4 {
			return fmt.Errorf("volume %q lacks a field", value)
		}
		v := volume{Mount: f[2], Name: f[3]}
		if _, err := fmt.Sscanf(value, "%d:%d:", &v.UsedMB, &v.TotalMB); err != nil {
			return err
		}
		if err := unescape(&v.Mount, &v.Name); err != nil {
			return err
		}
		l.Volumes = append(l.Volumes, v)
	case "sensor":
		temp, rest, _ := strings.Cut(value, ":")
		critical, name, _ := strings.Cut(rest, ":")
		s := sensor{Name: name}
		if err := unescape(&s.Name); err != nil {
			return err
		}
		var err error
		if s.Temp, err = strconv.ParseFloat(temp, 64); err != nil {
			return err
		}
		if critical != "" {
			s.Critical = new(float64)
			if *s.Critical, err = strconv.ParseFloat(critical, 64); err != nil {
				return err
			}
		}
		l.Sensors = append(l.Sensors, s)
	}
	return nil
}

func unescape(fields ...*string) error {
	for _, f := range fields {
		s, err := url.PathUnescape(*f)
		if err != nil {
			return err
		}
		*f = s
	}
	return nil
}

type peer struct {
	HostName, DNSName, CurAddr, Relay string
	TailscaleIPs                      []string
	Online                            bool
}

var tailnet struct {
	sync.Mutex
	peers map[string]peer
}

func followTailnet(logger *log.Logger) {
	for {
		readTailnet(logger)
		time.Sleep(tailnetEvery)
	}
}

func readTailnet(logger *log.Logger) {
	var status struct{ Peer map[string]peer }
	ctx, cancel := context.WithTimeout(context.Background(), tailscaleWait)
	defer cancel()
	if out, err := exec.CommandContext(ctx, "tailscale", "status", "--json").Output(); err == nil {
		if err := json.Unmarshal(out, &status); err != nil {
			logger.Printf("tailscale's status is unreadable: %v", err)
		}
	}
	tailnet.Lock()
	defer tailnet.Unlock()
	tailnet.peers = status.Peer
}

func relayed(ssh string) bool {
	host := ssh[strings.LastIndex(ssh, "@")+1:]
	tailnet.Lock()
	defer tailnet.Unlock()
	for _, p := range tailnet.peers {
		if p.HostName == host || strings.HasPrefix(p.DNSName, host+".") || slices.Contains(p.TailscaleIPs, host) {
			return p.Online && p.CurAddr == "" && p.Relay != ""
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
