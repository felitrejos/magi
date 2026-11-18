package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os/exec"
	"slices"
	"strconv"
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
	Arch        string    `json:"arch"`
	Tools       []string  `json:"tools"`
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
	Error    *string        `json:"error"`
	Sessions []agentSession `json:"sessions"`
	Probed   bool           `json:"-"`
}

type agentSession struct {
	CPU    float64 `json:"cpu"`
	Memory float64 `json:"memory"`
}

type fleetData struct {
	Machines []fleetMachine `json:"machines"`
	Plans    []plan         `json:"plans"`
}

func (m fleetMachine) busy() float64 {
	busy := m.CPU
	if len(m.Load) > 0 && m.Cores > 0 {
		busy = math.Max(busy, m.Load[0]/float64(m.Cores))
	}
	return busy
}

func (m fleetMachine) measured() bool {
	return m.Live || m.Probed && m.Cores > 0
}

func (m fleetMachine) describe() string {
	if m.Probed && m.Cores > 0 {
		parts := []string{fmt.Sprintf("load %.2f on %d cores", m.Load[0], m.Cores)}
		if m.MemoryTotal > 0 {
			parts = append(parts, fmt.Sprintf("memory %.1f of %.1f GB", m.MemoryUsed/1e9, m.MemoryTotal/1e9))
		}
		parts = append(parts, fmt.Sprintf("%d Claude sessions working", m.Agents.Working))
		return strings.Join(parts, ", ") + ", read over SSH"
	}
	if !m.Live {
		probed := "so its load is unknown; its system and tools were read over SSH"
		if m.Error == nil || *m.Error == "" {
			if m.Probed {
				return "the overview doesn't report it, " + probed
			}
			return "the overview: not answering (no answer)"
		}
		text := "the overview: not answering (" + *m.Error + ")"
		if m.Probed {
			text += ", " + probed
		}
		return text
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

func readFleet(ctx context.Context) (fleetData, error) {
	var fleet fleetData
	path, err := exec.LookPath("magi-overview")
	if err != nil {
		return fleet, errors.New("magi-overview isn't installed here")
	}
	ctx, cancel := context.WithTimeout(ctx, fleetWait)
	defer cancel()
	out, err := run(ctx, path, "fleet", "--json")
	if err != nil {
		return fleet, fmt.Errorf("magi-overview fleet failed: %w", err)
	}
	if err := json.Unmarshal(out, &fleet); err != nil {
		return fleetData{}, fmt.Errorf("magi-overview fleet: %w", err)
	}
	if len(fleet.Machines) == 0 {
		return fleet, errors.New("magi-overview fleet listed no machines")
	}
	return fleet, nil
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
	Plans     []plan
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
	data, err := readFleet(ctx)
	fleet := data.Machines
	view.FleetErr, view.Plans = err, data.Plans
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
			wg.Go(func() { view.Machines[i].probe(ctx) })
		}
		wg.Wait()
	}
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

func (m *machineInfo) probe(ctx context.Context) {
	if m.Fleet != nil && m.Fleet.Live {
		m.Reachable = reachable(ctx, m.Host)
		return
	}
	probed, err := probePlatform(ctx, m.Name, m.Host)
	m.Reachable = err
	if err == nil {
		if m.Fleet != nil {
			probed.Error = m.Fleet.Error
		}
		m.Fleet = probed
	}
}

const platformScript = `PATH="$PATH:$HOME/.local/bin:$HOME/go/bin:/opt/homebrew/bin:/usr/local/bin"
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
	page=$(sysctl -n hw.pagesize)
	free=$(vm_stat | awk '/Pages (free|inactive|speculative)/ { gsub("\\.", "", $NF); n += $NF } END { print n }')
	echo "$(sysctl -n hw.memsize) $((free * page))"
else
	getconf _NPROCESSORS_ONLN
	cut -d ' ' -f 1-3 /proc/loadavg
	awk '/^MemTotal:/ { t = $2 } /^MemAvailable:/ { a = $2 } END { print t * 1024, a * 1024 }' /proc/meminfo
fi
working=0
for f in "$HOME"/.claude/sessions/*.json; do
	[ -f "$f" ] && grep -q '"status": *"busy"' "$f" || continue
	pid=${f##*/}
	kill -0 "${pid%.json}" 2>/dev/null && working=$((working + 1))
done
echo "$working"`

func probePlatform(ctx context.Context, name string, h Host) (*fleetMachine, error) {
	ctx, cancel := context.WithTimeout(ctx, reachWait)
	defer cancel()
	r, err := newRemote(ctx, h.SSH)
	if err != nil {
		return nil, err
	}
	out, err := sh(r, platformScript)
	if err != nil {
		return nil, err
	}
	return parsePlatform(name, string(out)), nil
}

func parsePlatform(name, out string) *fleetMachine {
	lines := strings.Split(strings.TrimSpace(out), "\n")
	m := &fleetMachine{Name: name, Probed: true}
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
	m.Agents.Working = working
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
	Host  string
	Why   string
	Probe bool
}

func (view fleetView) candidates() (picks []choice, skipped []string) {
	unknown := "load unknown"
	if view.FleetErr != nil {
		unknown += " (" + view.FleetErr.Error() + ")"
	} else {
		unknown += " (the overview doesn't report it)"
	}
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
			picks = append(picks, choice{Host: m.Name, Why: "the first machine in your list that answers; " + unknown, Probe: true})
		}
	}
	return picks, skipped
}

func chooseHosts(ctx context.Context, requested string, agents bool) ([]choice, []string, []plan, error) {
	cfg, path, err := loadConfig()
	if err != nil {
		return nil, nil, nil, err
	}
	if len(cfg.Hosts) == 0 {
		return nil, nil, nil, fmt.Errorf("no hosts in %s; run magi init", path)
	}
	if requested != "" {
		if _, err := loadHost(requested); err != nil {
			return nil, nil, nil, err
		}
		var plans []plan
		if agents {
			data, _ := readFleet(ctx)
			plans = data.Plans
		}
		return []choice{{Host: requested, Why: "the machine you asked for"}}, nil, plans, nil
	}
	view := surveyFleet(ctx, cfg, true, agents)
	picks, skipped := view.candidates()
	return picks, skipped, view.Plans, nil
}

func (m fleetMachine) sessionLoad() string {
	if len(m.Sessions) == 0 {
		return ""
	}
	var cpu, memory float64
	for _, s := range m.Sessions {
		cpu += s.CPU
		memory += s.Memory
	}
	return fmt.Sprintf("its %d agent sessions use %.1f cores and %s", len(m.Sessions), cpu, formatMemory(memory))
}
