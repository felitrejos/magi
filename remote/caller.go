package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"runtime"
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

func callerOf(ctx context.Context, pid int) (caller, error) {
	started, err := processStart(ctx, pid)
	if err != nil {
		return caller{}, fmt.Errorf("finding when process %d started: %w", pid, err)
	}
	return caller{PID: pid, Started: started.Unix()}, nil
}

func procStat(pid int) ([]string, error) {
	stat, err := os.ReadFile("/proc/" + strconv.Itoa(pid) + "/stat")
	if err != nil {
		return nil, err
	}
	end := bytes.LastIndexByte(stat, ')')
	fields := strings.Fields(string(stat[end+1:]))
	if end < 0 || len(fields) < 20 {
		return nil, fmt.Errorf("unexpected /proc/%d/stat", pid)
	}
	return fields, nil
}

func processStart(ctx context.Context, pid int) (time.Time, error) {
	if runtime.GOOS != "linux" {
		cmd := exec.CommandContext(ctx, "ps", "-o", "lstart=", "-p", strconv.Itoa(pid))
		cmd.Env = append(os.Environ(), "TZ=UTC", "LC_ALL=C")
		out, err := cmd.Output()
		if err != nil {
			return time.Time{}, err
		}
		return time.Parse("Mon Jan 2 15:04:05 2006", strings.Join(strings.Fields(string(out)), " "))
	}
	fields, err := procStat(pid)
	if err != nil {
		return time.Time{}, err
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

func bootTime() (time.Time, error) {
	stat, err := os.ReadFile("/proc/stat")
	if err != nil {
		return time.Time{}, err
	}
	for line := range strings.Lines(string(stat)) {
		if rest, ok := strings.CutPrefix(line, "btime "); ok {
			seconds, err := strconv.ParseInt(strings.TrimSpace(rest), 10, 64)
			return time.Unix(seconds, 0), err
		}
	}
	return time.Time{}, errors.New("no btime in /proc/stat")
}

func parentOf(pid int) (int, error) {
	if runtime.GOOS != "linux" {
		out, err := run(context.Background(), "ps", "-o", "ppid=", "-p", strconv.Itoa(pid))
		if err != nil {
			return 0, err
		}
		return strconv.Atoi(strings.TrimSpace(string(out)))
	}
	fields, err := procStat(pid)
	if err != nil {
		return 0, err
	}
	return strconv.Atoi(fields[1])
}

func isAncestor(pid int) (bool, error) {
	for p := os.Getppid(); p > 1; {
		if p == pid {
			return true, nil
		}
		parent, err := parentOf(p)
		if err != nil {
			return false, err
		}
		p = parent
	}
	return false, nil
}
