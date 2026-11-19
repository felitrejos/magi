package main

import (
	"errors"
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"time"
)

const busyLimit = 0.9

type Budget struct {
	MaxAgents  int
	MinFree    float64
	QuietStart int
	QuietEnd   int
	Quiet      bool
}

var memoryUnits = map[string]float64{"": 1, "K": 1 << 10, "M": 1 << 20, "G": 1 << 30, "T": 1 << 40}

var memoryPattern = regexp.MustCompile(`^(\d+(?:\.\d+)?)\s*([KMGT]?)(?:I?B)?$`)

func parseMemory(text string) (float64, error) {
	m := memoryPattern.FindStringSubmatch(strings.ToUpper(strings.TrimSpace(text)))
	if m == nil {
		return 0, fmt.Errorf("%q isn't an amount of memory; write it like \"4G\" or \"512M\"", text)
	}
	n, err := strconv.ParseFloat(m[1], 64)
	if err != nil {
		return 0, err
	}
	return n * memoryUnits[m[2]], nil
}

func parseClock(text string) (int, error) {
	t, err := time.Parse("15:04", strings.TrimSpace(text))
	if err != nil {
		return 0, fmt.Errorf("%q isn't a time of day like 22:00", text)
	}
	return t.Hour()*60 + t.Minute(), nil
}

func (h Host) budget() (Budget, error) {
	b := Budget{MaxAgents: h.MaxAgents}
	if h.MaxAgents < 0 {
		return b, errors.New("max_agents can't be negative")
	}
	if h.MinFreeMemory != "" {
		free, err := parseMemory(h.MinFreeMemory)
		if err != nil {
			return b, fmt.Errorf("min_free_memory: %w", err)
		}
		b.MinFree = free
	}
	if h.QuietHours != "" {
		from, to, ok := strings.Cut(h.QuietHours, "-")
		if !ok {
			return b, fmt.Errorf("quiet_hours: %q isn't a window like \"22:00-07:00\"", h.QuietHours)
		}
		var err error
		if b.QuietStart, err = parseClock(from); err != nil {
			return b, fmt.Errorf("quiet_hours: %w", err)
		}
		if b.QuietEnd, err = parseClock(to); err != nil {
			return b, fmt.Errorf("quiet_hours: %w", err)
		}
		b.Quiet = b.QuietStart != b.QuietEnd
	}
	return b, nil
}

func (b Budget) quietAt(now time.Time) bool {
	if !b.Quiet {
		return false
	}
	minute := now.Hour()*60 + now.Minute()
	if b.QuietStart < b.QuietEnd {
		return minute >= b.QuietStart && minute < b.QuietEnd
	}
	return minute >= b.QuietStart || minute < b.QuietEnd
}

func formatMemory(bytes float64) string {
	return fmt.Sprintf("%.1f GB", bytes/(1<<30))
}

func clockText(minute int) string {
	return fmt.Sprintf("%02d:%02d", minute/60, minute%60)
}

func (b Budget) String() string {
	var parts []string
	if b.MaxAgents > 0 {
		parts = append(parts, fmt.Sprintf("at most %d working agents", b.MaxAgents))
	}
	if b.MinFree > 0 {
		parts = append(parts, fmt.Sprintf("at least %s free memory", formatMemory(b.MinFree)))
	}
	if b.Quiet {
		parts = append(parts, fmt.Sprintf("nothing new %s-%s", clockText(b.QuietStart), clockText(b.QuietEnd)))
	}
	if len(parts) == 0 {
		return "no budget"
	}
	return strings.Join(parts, ", ")
}

func (b Budget) noRoom(now time.Time, f *fleetMachine, agents bool) string {
	if b.quietAt(now) {
		return fmt.Sprintf("it's within its quiet hours (%s-%s)", clockText(b.QuietStart), clockText(b.QuietEnd))
	}
	if f == nil || !f.measured() {
		return ""
	}
	if agents && b.MaxAgents > 0 && f.Working >= b.MaxAgents {
		return fmt.Sprintf("%d agents are working there, its budget is %d", f.Working, b.MaxAgents)
	}
	if free := f.MemoryTotal - f.MemoryUsed; b.MinFree > 0 && f.MemoryTotal > 0 && free < b.MinFree {
		return fmt.Sprintf("%s of memory is free there, its budget needs %s", formatMemory(free), formatMemory(b.MinFree))
	}
	if busy := f.busy(); busy >= busyLimit {
		return fmt.Sprintf("it's %.0f%% busy", busy*100)
	}
	return ""
}

type noRoomError struct {
	Host   string
	Reason string
}

func (e *noRoomError) Error() string {
	return fmt.Sprintf("%s has no room: %s", e.Host, e.Reason)
}
