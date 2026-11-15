package main

import (
	"context"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"time"
)

type caller struct {
	PID     int
	Started int64
}

var self caller

const clockTicks = 100

func callerOf(ctx context.Context, pid int) caller {
	started, err := processStart(ctx, pid)
	if err != nil {
		return caller{PID: pid}
	}
	return caller{PID: pid, Started: started.Unix()}
}

func processStart(ctx context.Context, pid int) (time.Time, error) {
	if stat, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/stat"); err == nil {
		fields := strings.Fields(string(stat[strings.LastIndexByte(string(stat), ')')+1:]))
		if len(fields) < 20 {
			return time.Time{}, os.ErrInvalid
		}
		ticks, err := strconv.ParseInt(fields[19], 10, 64)
		if err != nil {
			return time.Time{}, err
		}
		boot, err := bootTime()
		if err != nil {
			return time.Time{}, err
		}
		return boot.Add(time.Duration(ticks) * time.Second / clockTicks), nil
	}
	cmd := exec.CommandContext(ctx, "ps", "-o", "lstart=", "-p", strconv.Itoa(pid))
	cmd.Env = append(os.Environ(), "TZ=UTC", "LC_ALL=C")
	out, err := cmd.Output()
	if err != nil {
		return time.Time{}, err
	}
	return time.Parse("Mon Jan 2 15:04:05 2006", strings.Join(strings.Fields(string(out)), " "))
}

func bootTime() (time.Time, error) {
	stat, err := os.ReadFile("/proc/stat")
	if err != nil {
		return time.Time{}, err
	}
	for _, line := range strings.Split(string(stat), "\n") {
		if rest, ok := strings.CutPrefix(line, "btime "); ok {
			seconds, err := strconv.ParseInt(strings.TrimSpace(rest), 10, 64)
			return time.Unix(seconds, 0), err
		}
	}
	return time.Time{}, os.ErrNotExist
}
