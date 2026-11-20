package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func readLive(t *testing.T, name string) liveMachine {
	t.Helper()
	path, err := livePath(name)
	if err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	var live liveMachine
	json.Unmarshal(data, &live)
	return live
}

const burn = `i=0; while [ "$i" -lt 300000 ]; do i=$((i + 1)); done`

func (f *fixture) worker(script string) int {
	f.t.Helper()
	done := filepath.Join(f.t.TempDir(), "done")
	cmd := exec.Command("sh", "-c", script, "sh", done)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL); cmd.Wait() })
	if !waitForFile(f.t, done, 30*time.Second) {
		f.t.Fatal("the worker never finished its work")
	}
	return cmd.Process.Pid
}

func usedBy(t *testing.T, stats string, pid int) (seconds float64, memoryMB int) {
	t.Helper()
	for _, field := range strings.Fields(stats) {
		if _, err := fmt.Sscanf(field, fmt.Sprintf("%d:%%g:%%d", pid), &seconds, &memoryMB); err == nil {
			return seconds, memoryMB
		}
	}
	t.Fatalf("stats line %q lacks process %d", stats, pid)
	return 0, 0
}

func TestWatchStreamsWhatChanges(t *testing.T) {
	f := newFixture(t)
	server := f.serverHome()
	pid := f.worker(`sh -c '` + burn + `; : > "$0"; exec sleep 60' "$1" & wait`)
	id := "0f0f0f0f-1111-2222-3333-555555555555"
	busySince := time.Now().Add(-time.Minute).Truncate(time.Millisecond)
	session := func(status string, at time.Time) {
		f.write(filepath.Join(server, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"%s","cwd":"/srv/api","name":"api","status":"%s","statusUpdatedAt":%d}`, pid, id, status, at.UnixMilli()))
	}
	session("busy", busySince)
	r := f.remoteHost()
	w := newWatcher("server", "", r, log.New(io.Discard, "", 0), "")
	cmd := r.Command(watchScript, "", "claude")
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	lines := make(chan string)
	go func() {
		for s := bufio.NewScanner(stdout); s.Scan(); {
			lines <- s.Text()
		}
	}()
	var heard []string
	await := func(kind string, n int) {
		t.Helper()
		for timeout := time.After(15 * time.Second); n > 0; {
			select {
			case line := <-lines:
				heard = append(heard, line)
				w.hear(line, time.Now())
				if strings.HasPrefix(line, kind) {
					n--
				}
			case <-timeout:
				t.Fatalf("no %s line; heard %q", kind, heard)
			}
		}
	}
	await("sessions", 1)
	await("stats", 1)
	if used, memory := usedBy(t, heard[len(heard)-1], pid); used < 0.1 || memory < 1 {
		t.Errorf("stats line = %q, want the session's tree summed with its child's CPU and memory", heard[len(heard)-1])
	}
	now := time.Now()
	w.hear(fmt.Sprintf("stats 10 1.5 100 200 300 400 0 0 1794928938 %d:10:150", pid), now)
	w.hear(fmt.Sprintf("stats 10 1.5 100 200 300 400 0 0 1794928938 %d:13:150", pid), now.Add(3*time.Second))
	live := readLive(t, "server")
	if len(live.Sessions) != 1 {
		t.Fatalf("live sessions = %+v", live.Sessions)
	}
	s := live.Sessions[0]
	if s.ID != id || s.State != "busy" || !s.Since.Equal(busySince) || s.Title != "api" || s.Folder != "/srv/api" {
		t.Errorf("live session = %+v, want %s busy since %s", s, id, busySince)
	}
	if s.MemoryMB != 150 || s.CPU < 90 || s.CPU > 110 {
		t.Errorf("session uses %d%% CPU and %d MB, want about 100%% and 150 MB", s.CPU, s.MemoryMB)
	}
	if !live.Answering || live.Cores != 10 || live.Memory.TotalMB != 200 || live.Disk.TotalMB != 400 {
		t.Errorf("live machine = %+v", live)
	}
	idleSince := time.Now().Truncate(time.Millisecond)
	session("idle", idleSince)
	await("sessions", 1)
	if s := readLive(t, "server").Sessions[0]; s.State != "idle" || !s.Since.Equal(idleSince) {
		t.Errorf("after going idle the live session = %+v", s)
	}
	var sessionLines []string
	for _, line := range heard {
		if strings.HasPrefix(line, "claude") {
			sessionLines = append(sessionLines, line)
		}
	}
	if len(sessionLines) != 2 || !strings.Contains(sessionLines[0], `"busy"`) || !strings.Contains(sessionLines[1], `"idle"`) {
		t.Errorf("session lines = %q, want one snapshot when busy and one when idle", sessionLines)
	}
}

func TestWatchCountsTheCPUOfChildrenThatExited(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("only Linux shows other processes how much CPU their exited children used")
	}
	f := newFixture(t)
	server := f.serverHome()
	pid := f.worker(`(` + burn + `); : > "$1"; sleep 60; :`)
	f.write(filepath.Join(server, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"0f0f0f0f-1111-2222-3333-555555555556","cwd":"/srv/api","status":"idle"}`, pid))
	stats := firstStats(t, f.remoteHost())
	if used, _ := usedBy(t, stats, pid); used < 0.1 {
		t.Errorf("stats line = %q, want the session's CPU to count its child that exited", stats)
	}
}

func firstStats(t *testing.T, m sh.Machine) string {
	t.Helper()
	cmd := m.Command(watchScript, "", "claude")
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	for lines := bufio.NewScanner(stdout); lines.Scan(); {
		if strings.HasPrefix(lines.Text(), "stats") {
			return lines.Text()
		}
	}
	t.Fatal("the watcher never reported stats")
	return ""
}

func TestAMacReportsItsMemoryPressureSwapBootTimeAndBattery(t *testing.T) {
	f := newFixture(t)
	f.tool("uname", "#!/bin/sh\necho Darwin\n")
	f.tool("sysctl", macSysctl)
	f.tool("pmset", "#!/bin/sh\nprintf \"Now drawing from 'Battery Power'\\n -InternalBattery-0 (id=4653155)\\t85%%; discharging; 4:10 remaining present: true\\n\"\n")
	w := newWatcher("mac", "", nil, log.New(io.Discard, "", 0), "")
	w.hear(firstStats(t, f.machine()), time.Now())
	live := readLive(t, "mac")
	if live.Pressure != "warning" || live.Swap != (space{UsedMB: 1095, TotalMB: 2048}) || !live.BootedAt.Equal(time.Unix(1794928938, 0)) {
		t.Errorf("pressure %q, swap %+v, booted at %s; want warning, 1095 of 2048 MB, %s", live.Pressure, live.Swap, live.BootedAt, time.Unix(1794928938, 0))
	}
	if live.Battery == nil || *live.Battery != (battery{Percent: 85, OnBattery: true}) {
		t.Errorf("battery = %+v, want 85%% and on battery", live.Battery)
	}
}

func TestALinuxMachineReportsItsMemoryPressureSwapBootTimeAndBattery(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("reads this machine's /proc")
	}
	f := newFixture(t)
	f.tool("grep", `#!/bin/sh
printf '%s\n' /sys/class/power_supply/AC/type:Mains /sys/class/power_supply/AC/online:1 /sys/class/power_supply/BAT0/type:Battery /sys/class/power_supply/BAT0/capacity:64 /sys/class/power_supply/BAT0/status:Charging /sys/class/power_supply/hidpp_battery_0/type:Battery /sys/class/power_supply/hidpp_battery_0/scope:Device /sys/class/power_supply/hidpp_battery_0/capacity:10
`)
	w := newWatcher("linux", "", nil, log.New(io.Discard, "", 0), "")
	w.hear(firstStats(t, f.machine()), time.Now())
	live := readLive(t, "linux")
	var swapKB, uptime float64
	meminfo, _ := os.ReadFile("/proc/meminfo")
	for _, line := range strings.Split(string(meminfo), "\n") {
		fmt.Sscanf(line, "SwapTotal: %g kB", &swapKB)
	}
	up, _ := os.ReadFile("/proc/uptime")
	fmt.Sscan(string(up), &uptime)
	pressure := ""
	if psi, err := os.ReadFile("/proc/pressure/memory"); err == nil {
		var avg10, avg60 float64
		fmt.Sscanf(string(psi), "some avg10=%g avg60=%g", &avg10, &avg60)
		pressure = "normal"
		if avg60 >= 10 {
			pressure = "warning"
		}
		if avg60 >= 30 {
			pressure = "critical"
		}
	}
	if booted := time.Now().Add(-time.Duration(uptime * float64(time.Second))); live.BootedAt.Sub(booted).Abs() > 2*time.Second {
		t.Errorf("booted at %s, want about %s", live.BootedAt, booted)
	}
	if live.Swap.TotalMB != int(math.Round(swapKB/1024)) || live.Swap.UsedMB > live.Swap.TotalMB || live.Pressure != pressure {
		t.Errorf("swap %+v and pressure %q, want %.0f MB of swap and pressure %q", live.Swap, live.Pressure, swapKB/1024, pressure)
	}
	if live.Battery == nil || *live.Battery != (battery{Percent: 64, Charging: true}) {
		t.Errorf("battery = %+v, want the system battery at 64%%, charging, on mains", live.Battery)
	}
}

func TestAMachineReachedThroughATailscaleRelayIsMarkedRelayed(t *testing.T) {
	f := newFixture(t)
	f.tool("tailscale", `#!/bin/sh
[ "$*" = "status --json" ] && echo '{"Peer":{"nodekey:1":{"HostName":"homelab","DNSName":"homelab.tail1.ts.net.","CurAddr":""},"nodekey:2":{"HostName":"Studio","DNSName":"studio.tail1.ts.net.","CurAddr":"192.168.1.20:41641"}}}'
`)
	for ssh, want := range map[string]bool{"me@homelab": true, "homelab.tail1.ts.net": true, "studio": false, "Studio": false, "elsewhere": false} {
		w := newWatcher(ssh, ssh, nil, log.New(io.Discard, "", 0), "")
		w.hear("stats 10 1.5 100 200 300 400 0 0 1794928938", time.Now())
		if got := readLive(t, ssh).Relayed; got != want {
			t.Errorf("%s reads as relayed %t, want %t", ssh, got, want)
		}
	}
}

func TestWatchMarksAMachineThatStopsAnswering(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	r, err := newRemote(ctx, Host{SSH: "fakehost", Claude: "claude"})
	if err != nil {
		t.Fatal(err)
	}
	stopped := make(chan error)
	go func() { stopped <- newWatcher("server", "", r, log.New(io.Discard, "", 0), "").follow("") }()
	for deadline := time.Now().Add(10 * time.Second); !readLive(t, "server").Answering; time.Sleep(100 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the machine was never marked as answering")
		}
	}
	cancel()
	<-stopped
	if live := readLive(t, "server"); live.Answering || live.Heard.IsZero() {
		t.Errorf("after the connection dropped the live file says %+v", live)
	}
}

func TestStoppingTheWatcherMarksEveryMachineAsNotAnswering(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	f.config("local_roots = [\"~/laptop\"]\n[hosts.server]\nssh = \"fakehost\"\nclaude = \"claude\"\nroots = [\"~/developer\"]\n")
	cmd := exec.Command(os.Args[0])
	cmd.Env = append(os.Environ(), "MAGI_TEST_RUN=watch")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	names := []string{f.machine().Name(), "server"}
	for deadline := time.Now().Add(15 * time.Second); !readLive(t, names[0]).Answering || !readLive(t, names[1]).Answering; time.Sleep(100 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the machines were never marked as answering")
		}
	}
	loops := descendants(cmd.Process.Pid)
	t.Cleanup(func() {
		for _, pid := range loops {
			syscall.Kill(pid, syscall.SIGKILL)
		}
	})
	cmd.Process.Signal(syscall.SIGTERM)
	cmd.Wait()
	for _, name := range names {
		if readLive(t, name).Answering {
			t.Errorf("after the watcher stopped, %s still reads as answering", name)
		}
	}
}

func TestTheLiveFileSaysHowToOpenASession(t *testing.T) {
	f := newFixture(t)
	f.isolatedTmux()
	server := f.serverHome()
	out, err := exec.Command("tmux", "new-session", "-d", "-P", "-F", "#{pane_pid} #{window_id}", "sleep 60").Output()
	if err != nil {
		t.Fatal(err)
	}
	pid, window, _ := strings.Cut(strings.TrimSpace(string(out)), " ")
	f.write(filepath.Join(server, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"0f0f0f0f-1111-2222-3333-555555555559","cwd":"/srv/api","status":"idle","bridgeSessionId":"session_42"}`)
	go newWatcher("server", "me@server", f.remoteHost(), log.New(io.Discard, "", 0), "").follow("")
	for deadline := time.Now().Add(10 * time.Second); ; time.Sleep(100 * time.Millisecond) {
		if live := readLive(t, "server"); len(live.Sessions) == 1 {
			if s := live.Sessions[0]; s.RemoteControl != "https://claude.ai/code/session_42" || s.Tmux != window || live.SSH != "me@server" {
				t.Errorf("the session opens through %q and tmux window %q (want %s), the machine over ssh %q", s.RemoteControl, s.Tmux, window, live.SSH)
			}
			return
		}
		if time.Now().After(deadline) {
			t.Fatal("the session never reached the live file")
		}
	}
}

func TestANewWatchLoopStopsOrphanedLoopsForItsMachine(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	r := f.remoteHost()
	line := func(name string) string {
		args := r.Command(`: "$HOME/.magi/watchers"; while :; do sleep 1; done`, name, "claude").Args
		return args[len(args)-1]
	}
	orphan := func(name string) int {
		out, err := exec.Command("sh", "-c", `sh -c "$1" >/dev/null 2>&1 & echo $!`, "sh", line(name)).Output()
		if err != nil {
			t.Fatal(err)
		}
		pid, err := strconv.Atoi(strings.TrimSpace(string(out)))
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { syscall.Kill(pid, syscall.SIGKILL) })
		return pid
	}
	old, other := orphan("laptop"), orphan("other")
	connected := exec.Command("sh", "-c", line("laptop"))
	if err := connected.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { connected.Process.Kill(); connected.Wait() })
	cmd := r.Command(watchScript, "laptop", "claude")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	running := func(pid int) bool {
		if stat, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid)); err == nil {
			return !strings.Contains(string(stat), ") Z ")
		}
		return syscall.Kill(pid, 0) == nil
	}
	for deadline := time.Now().Add(10 * time.Second); running(old); time.Sleep(100 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatal("the orphaned loop for this machine is still running")
		}
	}
	if !running(other) || !running(connected.Process.Pid) {
		t.Error("the new loop stopped a loop for another machine or one that still has its connection")
	}
}

func TestTheWatcherRestartsForAnUpdateOnlyOnceNoSessionIsBeingTakenBack(t *testing.T) {
	newFixture(t)
	every := heartbeatEvery
	heartbeatEvery = 10 * time.Millisecond
	t.Cleanup(func() { heartbeatEvery = every })
	link, err := magiLink()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(link), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"magi-1", "magi-2"} {
		if err := os.WriteFile(filepath.Join(filepath.Dir(link), name), nil, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Symlink("magi-1", link); err != nil {
		t.Fatal(err)
	}
	taking.RLock()
	restarted := make(chan struct{})
	go heartbeat(log.New(io.Discard, "", 0), func() {
		taking.Unlock()
		close(restarted)
		runtime.Goexit()
	})
	time.Sleep(50 * time.Millisecond)
	if err := os.Remove(link); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("magi-2", link); err != nil {
		t.Fatal(err)
	}
	select {
	case <-restarted:
		t.Fatal("the watcher restarted while it was taking a session back")
	case <-time.After(200 * time.Millisecond):
	}
	taking.RUnlock()
	select {
	case <-restarted:
	case <-time.After(5 * time.Second):
		t.Fatal("the watcher never restarted for the update")
	}
}

func TestTheWatcherRestartsWhenAMachineIsAdded(t *testing.T) {
	f := newFixture(t)
	every := heartbeatEvery
	heartbeatEvery = 10 * time.Millisecond
	t.Cleanup(func() { heartbeatEvery = every })
	f.config("local_roots = [\"/tmp\"]\n")
	restarted := make(chan struct{})
	go heartbeat(log.New(io.Discard, "", 0), func() {
		taking.Unlock()
		close(restarted)
		runtime.Goexit()
	})
	time.Sleep(50 * time.Millisecond)
	f.config("local_roots = [\"/tmp\"]\n[hosts.server]\nssh = \"server\"\n")
	select {
	case <-restarted:
	case <-time.After(5 * time.Second):
		t.Fatal("the watcher kept running without the new machine")
	}
}
