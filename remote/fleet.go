package main

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

const reachWait = 8 * time.Second

type fleetMachine struct {
	Name        string
	OS          string
	Arch        string
	Tools       []string
	Cores       int
	Load        float64
	MemoryUsed  float64
	MemoryTotal float64
	Working     int
}

func (m fleetMachine) busy() float64 {
	if m.Cores == 0 {
		return 0
	}
	return m.Load / float64(m.Cores)
}

func (m fleetMachine) measured() bool {
	return m.Cores > 0
}

func (m fleetMachine) describe() string {
	if !m.measured() {
		return "its system and tools were read over SSH, but not its load"
	}
	parts := []string{fmt.Sprintf("load %.2f on %d cores", m.Load, m.Cores)}
	if m.MemoryTotal > 0 {
		parts = append(parts, "memory "+formatMemory(m.MemoryUsed)+" of "+formatMemory(m.MemoryTotal))
	}
	parts = append(parts, fmt.Sprintf("%d Claude sessions working", m.Working))
	return strings.Join(parts, ", ")
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
	Machines []machineInfo
	Here     *fleetMachine
}

func surveyFleet(ctx context.Context, cfg Config, agents bool) fleetView {
	var view fleetView
	tasks, _ := listTasks()
	now := time.Now()
	for _, name := range cfg.Order {
		m := machineInfo{Name: name, Host: cfg.Hosts[name]}
		for _, t := range tasks {
			if t.Host == name && t.Outcome == nil {
				m.Tasks = append(m.Tasks, t)
			}
		}
		view.Machines = append(view.Machines, m)
	}
	var wg sync.WaitGroup
	for i := range view.Machines {
		wg.Go(func() {
			m := &view.Machines[i]
			m.Fleet, m.Reachable = probeMachine(ctx, m.Name, m.Host)
		})
	}
	wg.Go(func() {
		if local, err := sh.NewLocal(ctx); err == nil {
			view.Here, _ = probePlatform(local, local.Name())
		}
	})
	wg.Wait()
	for i := range view.Machines {
		m := &view.Machines[i]
		var err error
		if m.Budget, err = m.Host.budget(); err != nil {
			m.NoRoom = err.Error()
		} else {
			m.NoRoom = m.Budget.noRoom(now, m.Fleet, agents)
		}
	}
	return view
}

var probeMachine = func(ctx context.Context, name string, h Host) (*fleetMachine, error) {
	ctx, cancel := context.WithTimeout(ctx, reachWait)
	defer cancel()
	r, err := newRemote(ctx, h)
	if err != nil {
		return nil, err
	}
	return probePlatform(r, name)
}

func probePlatform(m sh.Machine, name string) (*fleetMachine, error) {
	out, err := sh.Run(m, platformScript, m.Claude())
	if err != nil {
		return nil, err
	}
	return parsePlatform(name, string(out)), nil
}

func parsePlatform(name, out string) *fleetMachine {
	lines := strings.Split(strings.TrimSpace(out), "\n")
	m := &fleetMachine{Name: name}
	if len(lines) > 0 {
		m.OS = strings.TrimSpace(lines[0])
	}
	if len(lines) > 1 {
		m.Arch = strings.TrimSpace(lines[1])
	}
	if len(lines) > 2 {
		m.Tools = strings.Fields(lines[2])
	}
	if len(lines) < 7 {
		return m
	}
	cores, coresErr := strconv.Atoi(strings.TrimSpace(lines[3]))
	load, loadErr := strconv.ParseFloat(firstField(lines[4]), 64)
	memory := strings.Fields(lines[5])
	working, workingErr := strconv.Atoi(strings.TrimSpace(lines[6]))
	if coresErr != nil || loadErr != nil || workingErr != nil || len(memory) != 2 || cores <= 0 {
		return m
	}
	total, totalErr := strconv.ParseFloat(memory[0], 64)
	available, availableErr := strconv.ParseFloat(memory[1], 64)
	if totalErr != nil || availableErr != nil {
		return m
	}
	m.Cores, m.Load, m.MemoryTotal, m.MemoryUsed = cores, load, total, total-available
	m.Working = working
	return m
}

func firstField(line string) string {
	fields := strings.Fields(line)
	if len(fields) == 0 {
		return ""
	}
	return fields[0]
}

type choice struct {
	Host string
	Why  string
}

func (view fleetView) candidates() (picks []choice, skipped []string) {
	for _, m := range view.Machines {
		switch {
		case m.Reachable != nil:
			skipped = append(skipped, fmt.Sprintf("%s isn't reachable (%v)", m.Name, m.Reachable))
		case m.NoRoom != "":
			skipped = append(skipped, fmt.Sprintf("%s has no room: %s", m.Name, m.NoRoom))
		case m.Fleet != nil && m.Fleet.measured():
			why := "the first machine in your list with room (" + m.Fleet.describe() + ")"
			if view.Here != nil && view.Here.busy() < m.Fleet.busy() {
				why += fmt.Sprintf("; this machine is lighter right now (%s)", view.Here.describe())
			}
			picks = append(picks, choice{Host: m.Name, Why: why})
		default:
			picks = append(picks, choice{Host: m.Name, Why: "the first machine in your list that answers; its load is unknown"})
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
		return nil, nil, fmt.Errorf("no hosts in %s; run magi init", path)
	}
	if requested != "" {
		if _, err := loadHost(requested); err != nil {
			return nil, nil, err
		}
		return []choice{{Host: requested, Why: "the machine you asked for"}}, nil, nil
	}
	view := surveyFleet(ctx, cfg, agents)
	picks, skipped := view.candidates()
	return picks, skipped, nil
}
