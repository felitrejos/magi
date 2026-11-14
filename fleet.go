package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"math"
	"os/exec"
	"slices"
	"strings"
	"sync"
	"time"
)

const (
	fleetWait = 6 * time.Second
	reachWait = 8 * time.Second
)

type fleetMachine struct {
	Name        string    `json:"name"`
	Here        bool      `json:"here"`
	Live        bool      `json:"live"`
	OS          string    `json:"os"`
	Cores       int       `json:"cores"`
	CPU         float64   `json:"cpu"`
	Load        []float64 `json:"load"`
	MemoryUsed  float64   `json:"memory_used"`
	MemoryTotal float64   `json:"memory_total"`
	Agents      struct {
		Working int `json:"working"`
		Waiting int `json:"waiting"`
		Idle    int `json:"idle"`
	} `json:"agents"`
	Error *string `json:"error"`
}

func (m fleetMachine) busy() float64 {
	busy := m.CPU
	if len(m.Load) > 0 && m.Cores > 0 {
		busy = math.Max(busy, m.Load[0]/float64(m.Cores))
	}
	return busy
}

func (m fleetMachine) memory() float64 {
	if m.MemoryTotal <= 0 {
		return 0
	}
	return m.MemoryUsed / m.MemoryTotal
}

func (m fleetMachine) describe() string {
	if !m.Live {
		reason := "no answer"
		if m.Error != nil && *m.Error != "" {
			reason = *m.Error
		}
		return "magi: not answering (" + reason + ")"
	}
	parts := []string{fmt.Sprintf("cpu %.0f%%", m.CPU*100)}
	if len(m.Load) > 0 {
		parts = append(parts, fmt.Sprintf("load %.2f on %d cores", m.Load[0], m.Cores))
	}
	if m.MemoryTotal > 0 {
		parts = append(parts, fmt.Sprintf("memory %.1f of %.1f GB", m.MemoryUsed/1e9, m.MemoryTotal/1e9))
	}
	parts = append(parts, fmt.Sprintf("agents %d working, %d waiting, %d idle", m.Agents.Working, m.Agents.Waiting, m.Agents.Idle))
	return strings.Join(parts, ", ")
}

var errNoMagi = errors.New("magi isn't installed here")

func readFleet(ctx context.Context) ([]fleetMachine, error) {
	path, err := exec.LookPath("magi")
	if err != nil {
		return nil, errNoMagi
	}
	ctx, cancel := context.WithTimeout(ctx, fleetWait)
	defer cancel()
	out, err := run(ctx, path, "fleet", "--json")
	if err != nil {
		return nil, fmt.Errorf("magi fleet failed: %w", err)
	}
	var fleet struct {
		Machines []fleetMachine `json:"machines"`
	}
	if err := json.Unmarshal(out, &fleet); err != nil {
		return nil, fmt.Errorf("magi fleet: %w", err)
	}
	if len(fleet.Machines) == 0 {
		return nil, errors.New("magi fleet listed no machines")
	}
	return fleet.Machines, nil
}

func hostAliases(name string, h Host) []string {
	ssh := h.SSH
	if _, after, ok := strings.Cut(ssh, "@"); ok {
		ssh = after
	}
	ssh, _, _ = strings.Cut(ssh, ":")
	short, _, _ := strings.Cut(ssh, ".")
	return []string{strings.ToLower(name), strings.ToLower(h.SSH), strings.ToLower(ssh), strings.ToLower(short)}
}

func fleetIndex(name string, h Host, fleet []fleetMachine) int {
	aliases := hostAliases(name, h)
	return slices.IndexFunc(fleet, func(m fleetMachine) bool {
		machine := strings.ToLower(m.Name)
		short, _, _ := strings.Cut(machine, ".")
		return !m.Here && (slices.Contains(aliases, machine) || slices.Contains(aliases, short))
	})
}

type machineInfo struct {
	Name      string
	Host      Host
	Fleet     *fleetMachine
	Reachable error
	Tasks     []*Task
}

type fleetView struct {
	Machines  []machineInfo
	Here      *fleetMachine
	Unmatched []fleetMachine
	FleetErr  error
}

func reachable(ctx context.Context, h Host) error {
	ctx, cancel := context.WithTimeout(ctx, reachWait)
	defer cancel()
	r, err := newRemote(ctx, h.SSH)
	if err != nil {
		return err
	}
	_, err = sh(r, "true")
	return err
}

func surveyFleet(ctx context.Context, cfg Config, probe bool) fleetView {
	var view fleetView
	fleet, err := readFleet(ctx)
	view.FleetErr = err
	used := map[int]bool{}
	tasks, _ := listTasks()
	for _, name := range slices.Sorted(maps.Keys(cfg.Hosts)) {
		h := cfg.Hosts[name]
		m := machineInfo{Name: name, Host: h}
		if i := fleetIndex(name, h, fleet); i >= 0 {
			m.Fleet = &fleet[i]
			used[i] = true
		}
		for _, t := range tasks {
			if t.Host == name && t.Outcome == nil {
				m.Tasks = append(m.Tasks, t)
			}
		}
		view.Machines = append(view.Machines, m)
	}
	for i := range fleet {
		switch {
		case fleet[i].Here:
			view.Here = &fleet[i]
		case !used[i]:
			view.Unmatched = append(view.Unmatched, fleet[i])
		}
	}
	if probe {
		var wg sync.WaitGroup
		for i := range view.Machines {
			wg.Go(func() { view.Machines[i].Reachable = reachable(ctx, view.Machines[i].Host) })
		}
		wg.Wait()
	}
	return view
}

type choice struct {
	Host string
	Why  string
}

func chooseHost(ctx context.Context, requested string) (choice, error) {
	cfg, path, err := loadConfig()
	if err != nil {
		return choice{}, err
	}
	if len(cfg.Hosts) == 0 {
		return choice{}, fmt.Errorf("no hosts in %s; run cielago init", path)
	}
	if requested != "" {
		if _, err := loadHost(requested); err != nil {
			return choice{}, err
		}
		return choice{Host: requested, Why: "the machine you asked for"}, nil
	}
	view := surveyFleet(ctx, cfg, false)
	if c, ok := leastLoaded(view); ok {
		return c, nil
	}
	reason := "magi knows none of the configured machines"
	if view.FleetErr != nil {
		reason = view.FleetErr.Error()
	}
	var failures []string
	for _, m := range view.Machines {
		if err := reachable(ctx, m.Host); err != nil {
			failures = append(failures, fmt.Sprintf("%s (%v)", m.Name, err))
			continue
		}
		return choice{Host: m.Name, Why: fmt.Sprintf("load unknown (%s), so the first configured machine that answers", reason)}, nil
	}
	return choice{}, fmt.Errorf("no configured machine answers: %s", strings.Join(failures, "; "))
}

func leastLoaded(view fleetView) (choice, bool) {
	var best *machineInfo
	for i := range view.Machines {
		m := &view.Machines[i]
		if m.Fleet == nil || !m.Fleet.Live || m.Reachable != nil {
			continue
		}
		if best == nil || m.Fleet.busy() < best.Fleet.busy() || m.Fleet.busy() == best.Fleet.busy() && m.Fleet.memory() < best.Fleet.memory() {
			best = m
		}
	}
	if best != nil {
		why := fmt.Sprintf("the least loaded configured machine (%s)", best.Fleet.describe())
		var unknown []string
		for _, m := range view.Machines {
			if m.Fleet == nil || !m.Fleet.Live {
				unknown = append(unknown, m.Name)
			}
		}
		if len(unknown) > 0 {
			why += fmt.Sprintf("; magi doesn't report the load of %s", strings.Join(unknown, ", "))
		}
		if view.Here != nil && view.Here.busy() < best.Fleet.busy() {
			why += fmt.Sprintf("; note this machine is lighter right now (%s)", view.Here.describe())
		}
		return choice{Host: best.Name, Why: why}, true
	}
	return choice{}, false
}
