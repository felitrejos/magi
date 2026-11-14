package main

import (
	"fmt"
	"strings"
	"time"
)

const (
	planRefuse = 0.9
	planWarn   = 0.75
)

type plan struct {
	Agent   string       `json:"agent"`
	Windows []planWindow `json:"windows"`
}

type planWindow struct {
	Label    string  `json:"label"`
	Used     float64 `json:"used"`
	ResetsAt int64   `json:"resets_at"`
}

func (w planWindow) describe(now time.Time) string {
	text := fmt.Sprintf("%s window is %.0f%% used", w.Label, w.Used*100)
	if w.ResetsAt > 0 {
		reset := time.Unix(w.ResetsAt, 0).Local()
		if reset.Sub(now) < 20*time.Hour {
			text += " and resets at " + reset.Format("15:04")
		} else {
			text += " and resets " + reset.Format("Mon 15:04")
		}
	}
	return text
}

type planVerdict struct {
	Warn   string
	Refuse string
}

func checkPlan(plans []plan, agent string, now time.Time) planVerdict {
	var full, near []string
	for _, p := range plans {
		if p.Agent != agent {
			continue
		}
		for _, w := range p.Windows {
			switch {
			case w.Used >= planRefuse:
				full = append(full, w.describe(now))
			case w.Used >= planWarn:
				near = append(near, w.describe(now))
			}
		}
	}
	var v planVerdict
	if len(full) > 0 {
		v.Refuse = fmt.Sprintf("the %s plan's %s; it's the same plan for every session on every machine, so more %s work now could run it out for all of them. Wait for the reset, or start it anyway with despite_plan if the user says so", agentName(agent), strings.Join(full, ", and its "), agentName(agent))
	}
	if len(near) > 0 {
		v.Warn = fmt.Sprintf("the %s plan's %s", agentName(agent), strings.Join(near, ", and its "))
	}
	return v
}

func agentName(agent string) string {
	if agent == "claude" {
		return "Claude"
	}
	return agent
}

func describePlans(plans []plan, now time.Time) string {
	var lines []string
	for _, p := range plans {
		var windows []string
		for _, w := range p.Windows {
			windows = append(windows, w.describe(now))
		}
		if len(windows) > 0 {
			lines = append(lines, fmt.Sprintf("%s plan (shared by all machines): %s", agentName(p.Agent), strings.Join(windows, "; ")))
		}
	}
	return strings.Join(lines, "\n")
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
