package main

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"
)

const reachWait = 8 * time.Second

type fleetMachine struct {
	Name        string
	OS          string
	Arch        string
	Tools       []string
	Cores       int
	Load        []float64
	MemoryUsed  float64
	MemoryTotal float64
	Working     int
}

func (m fleetMachine) busy() float64 {
	if len(m.Load) == 0 || m.Cores == 0 {
		return 0
	}
	return m.Load[0] / float64(m.Cores)
}

func (m fleetMachine) measured() bool {
	return m.Cores > 0
}

func (m fleetMachine) describe() string {
	if !m.measured() {
		return "its system and tools were read over SSH, but not its load"
	}
	parts := []string{fmt.Sprintf("load %.2f on %d cores", m.Load[0], m.Cores)}
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
	wg.Go(func() { view.Here = probeHere(ctx) })
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

func probeHere(ctx context.Context) *fleetMachine {
	local, err := newLocal(ctx)
	if err != nil {
		return nil
	}
	out, err := sh(local, platformScript, local.Claude())
	if err != nil {
		return nil
	}
	return parsePlatform(local.Name(), string(out))
}

const platformScript = `PATH="$PATH:$HOME/.local/bin:$HOME/go/bin:/opt/homebrew/bin:/usr/local/bin:/usr/sbin:/sbin"
claude=$1
case $claude in \~/*) claude=$HOME/${claude#\~/} ;; esac
if [ "$(uname -s)" = Darwin ]; then
	echo "macOS $(sw_vers -productVersion)"
else
	name=$(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")
	echo "${name:-$(uname -s)}"
fi
uname -m
for t in cargo go node python3 swift xcodebuild gcc clang make nvidia-smi; do
	command -v "$t" >/dev/null 2>&1 && printf '%s ' "$t"
done
bounded() { if command -v timeout >/dev/null 2>&1; then timeout 3 "$@"; else "$@"; fi; }
for t in docker podman; do
	command -v "$t" >/dev/null 2>&1 && bounded "$t" info >/dev/null 2>&1 && printf '%s ' "$t"
done
echo
if [ "$(uname -s)" = Darwin ]; then
	sysctl -n hw.ncpu
	sysctl -n vm.loadavg | tr -d '{}'
	sysctl -n hw.memsize hw.pagesize vm.page_free_count vm.page_pageable_external_count vm.page_purgeable_count | awk '{ v[NR] = $1 } END { printf "%.0f %.0f\n", v[1], (v[3] + v[4] + v[5]) * v[2] }'
else
	getconf _NPROCESSORS_ONLN
	cut -d ' ' -f 1-3 /proc/loadavg
	awk '/^MemTotal:/ { t = $2 } /^MemAvailable:/ { a = $2 } END { print t * 1024, a * 1024 }' /proc/meminfo
fi
listed=$("$claude" agents --json) || exit
printf '%s\n' "$listed" | awk '$1 == "\"status\":" && $2 ~ /^"busy"/ { n++ } END { print n + 0 }'`

var probeMachine = probePlatform

func probePlatform(ctx context.Context, name string, h Host) (*fleetMachine, error) {
	ctx, cancel := context.WithTimeout(ctx, reachWait)
	defer cancel()
	r, err := newRemote(ctx, h)
	if err != nil {
		return nil, err
	}
	out, err := sh(r, platformScript, r.Claude())
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
	m.Cores, m.Load, m.MemoryTotal, m.MemoryUsed = cores, []float64{load}, total, total-available
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
