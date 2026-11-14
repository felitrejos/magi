package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
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
	Budget    Budget
	Fleet     *fleetMachine
	Reachable error
	NoRoom    string
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

func surveyFleet(ctx context.Context, cfg Config, probe, agents bool) fleetView {
	var view fleetView
	fleet, err := readFleet(ctx)
	view.FleetErr = err
	used := map[int]bool{}
	tasks, _ := listTasks()
	now := time.Now()
	for _, name := range cfg.Order {
		h := cfg.Hosts[name]
		m := machineInfo{Name: name, Host: h}
		if i := fleetIndex(name, h, fleet); i >= 0 {
			m.Fleet = &fleet[i]
			used[i] = true
		}
		var err error
		if m.Budget, err = h.budget(); err != nil {
			m.NoRoom = err.Error()
		} else {
			m.NoRoom = m.Budget.noRoom(now, m.Fleet, agents)
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
	Host  string
	Why   string
	Probe bool
}

func (view fleetView) candidates() (picks []choice, skipped []string) {
	unknown := "load unknown"
	if view.FleetErr != nil {
		unknown += " (" + view.FleetErr.Error() + ")"
	} else {
		unknown += " (magi doesn't report it)"
	}
	for _, m := range view.Machines {
		switch {
		case m.Reachable != nil:
			skipped = append(skipped, fmt.Sprintf("%s isn't reachable (%v)", m.Name, m.Reachable))
		case m.NoRoom != "":
			skipped = append(skipped, fmt.Sprintf("%s has no room: %s", m.Name, m.NoRoom))
		case m.Fleet != nil && m.Fleet.Live:
			why := "the first machine in your list with room (" + m.Fleet.describe() + ")"
			if view.Here != nil && view.Here.busy() < m.Fleet.busy() {
				why += fmt.Sprintf("; this machine is lighter right now (%s)", view.Here.describe())
			}
			picks = append(picks, choice{Host: m.Name, Why: why})
		default:
			picks = append(picks, choice{Host: m.Name, Why: "the first machine in your list that answers; " + unknown, Probe: true})
		}
	}
	return picks, skipped
}

func chooseHosts(ctx context.Context, requested string, agents bool) ([]choice, []string, error) {
	cfg, path, err := loadConfig()
	if err != nil {
		return nil, nil, err
	}
	if len(cfg.Hosts) == 0 {
		return nil, nil, fmt.Errorf("no hosts in %s; run cielago init", path)
	}
	if requested != "" {
		if _, err := loadHost(requested); err != nil {
			return nil, nil, err
		}
		return []choice{{Host: requested, Why: "the machine you asked for"}}, nil, nil
	}
	picks, skipped := surveyFleet(ctx, cfg, false, agents).candidates()
	return picks, skipped, nil
}
