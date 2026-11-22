package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
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
	return heardStats(t, m, newWatcher("heard", "", nil, log.New(io.Discard, "", 0), ""), 1, 0)[0]
}

func heardStats(t *testing.T, m sh.Machine, w *watcher, n int, apart time.Duration) []string {
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
	at := time.Unix(1_800_000_000, 0)
	var stats []string
	for lines := bufio.NewScanner(stdout); lines.Scan(); {
		w.hear(lines.Text(), at)
		if strings.HasPrefix(lines.Text(), "stats") {
			if stats = append(stats, lines.Text()); len(stats) == n {
				return stats
			}
			at = at.Add(apart)
		}
	}
	t.Fatalf("the watcher reported %d stats lines, want %d", len(stats), n)
	return nil
}

func (f *fixture) served(name, fixture string) {
	f.t.Helper()
	path, err := filepath.Abs(filepath.Join("testdata", fixture))
	if err != nil {
		f.t.Fatal(err)
	}
	f.tool(name, "#!/bin/sh\nexec /bin/cat '"+path+"'\n")
}

func testdata(t *testing.T, name string) string {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("testdata", name))
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func (f *fixture) df(fixture string) {
	f.t.Helper()
	path, err := filepath.Abs(filepath.Join("testdata", fixture))
	if err != nil {
		f.t.Fatal(err)
	}
	f.tool("df", `#!/bin/sh
shift
if [ "$1" = "$HOME" ]; then exec head -2 '`+path+`'; fi
head -1 '`+path+`'
for m; do awk -v m="$m" '{ r = $0; sub(/^[^ ]+ +[^ ]+ +[^ ]+ +[^ ]+ +[^ ]+ +/, "", r) } NR > 1 && r == m' '`+path+`'; done
`)
}

func listed(live liveMachine) []string {
	var got []string
	if live.Top != nil {
		for _, p := range live.Top.CPU {
			got = append(got, fmt.Sprintf("cpu %d %s %d%% %dMB %s", p.PID, p.Name, p.CPU, p.MemoryMB, p.Session))
		}
		for _, p := range live.Top.Memory {
			got = append(got, fmt.Sprintf("memory %d %s %d%% %dMB %s", p.PID, p.Name, p.CPU, p.MemoryMB, p.Session))
		}
	}
	for _, p := range live.MemoryParts {
		got = append(got, fmt.Sprintf("part %s %d", p.Kind, p.MB))
	}
	for _, v := range live.Volumes {
		got = append(got, fmt.Sprintf("volume %s %s %d/%d", v.Name, v.Mount, v.UsedMB, v.TotalMB))
	}
	for _, i := range live.Interfaces {
		got = append(got, fmt.Sprintf("interface %s %d %d", i.Name, i.Received, i.Sent))
	}
	for _, s := range live.Sensors {
		got = append(got, fmt.Sprintf("sensor %s %g %s", s.Name, s.Temp, value(s.Critical)))
	}
	return got
}

func TestTheLiveFileListsEachSamplesProcessesMemoryVolumesInterfacesAndSensors(t *testing.T) {
	newFixture(t)
	w := newWatcher("server", "", nil, log.New(io.Discard, "", 0), "")
	w.live.Sessions = []liveEntry{{ID: "a", pid: 42}}
	start := time.Unix(1_800_000_000, 0)
	nics := ""
	for i := range 10 {
		nics += fmt.Sprintf(" nic=%d:%d:en%d", 1000*i, 500, i)
	}
	w.hear("stats 10 1 100 200 300 400 0 0 1794928938"+nics+" nic=7:7:utun0 part=app:300 part=free:100 sensor=51.0:100:Package%20id%200", start)
	if live := readLive(t, "server"); live.Top != nil || live.Interfaces != nil || len(live.MemoryParts) != 2 || len(live.Sensors) != 1 {
		t.Errorf("after one sample the live file lists %q, want parts and sensors but no processes or interfaces", listed(live))
	}
	nics, procs, volumes := "", "", ""
	for i := range 10 {
		nics += fmt.Sprintf(" nic=%d:%d:en%d", 4000*i, 500+30*i, i)
	}
	for i := range 9 {
		procs += fmt.Sprintf(" proc=%d:%g:%d:%d:Helper%%20(Renderer)%%3A%d", 100+i, 3*float64(i), 900-100*i, 42*(i%2), i)
		volumes += fmt.Sprintf(" vol=%d:%d:/Volumes/Disk%%20%d:Disk%%20%d", i, 1000*i, i, i)
	}
	w.hear("stats 10 1 100 200 300 400 0 0 1794928938"+nics+" nic=7:7:utun0 nic=10:10:awdl0"+procs+volumes+" part=app:250", start.Add(30*time.Second))
	want := []string{
		"cpu 108 Helper (Renderer):8 80% 100MB ", "cpu 107 Helper (Renderer):7 70% 200MB a", "cpu 106 Helper (Renderer):6 60% 300MB ", "cpu 105 Helper (Renderer):5 50% 400MB a",
		"cpu 104 Helper (Renderer):4 40% 500MB ", "cpu 103 Helper (Renderer):3 30% 600MB a", "cpu 102 Helper (Renderer):2 20% 700MB ", "cpu 101 Helper (Renderer):1 10% 800MB a",
		"memory 100 Helper (Renderer):0 0% 900MB ", "memory 101 Helper (Renderer):1 10% 800MB a", "memory 102 Helper (Renderer):2 20% 700MB ", "memory 103 Helper (Renderer):3 30% 600MB a",
		"memory 104 Helper (Renderer):4 40% 500MB ", "memory 105 Helper (Renderer):5 50% 400MB a", "memory 106 Helper (Renderer):6 60% 300MB ", "memory 107 Helper (Renderer):7 70% 200MB a",
		"part app 250",
		"volume Disk 8 /Volumes/Disk 8 8/8000", "volume Disk 7 /Volumes/Disk 7 7/7000", "volume Disk 6 /Volumes/Disk 6 6/6000", "volume Disk 5 /Volumes/Disk 5 5/5000",
		"volume Disk 4 /Volumes/Disk 4 4/4000", "volume Disk 3 /Volumes/Disk 3 3/3000", "volume Disk 2 /Volumes/Disk 2 2/2000", "volume Disk 1 /Volumes/Disk 1 1/1000",
		"interface en9 900 9", "interface en8 800 8", "interface en7 700 7", "interface en6 600 6",
		"interface en5 500 5", "interface en4 400 4", "interface en3 300 3", "interface en2 200 2",
	}
	if got := listed(readLive(t, "server")); !reflect.DeepEqual(got, want) {
		t.Errorf("the second sample lists\n%q\nwant\n%q", got, want)
	}
}

func TestAMacReportsItsMemoryPressureSwapBootTimeBatteryAndSamples(t *testing.T) {
	f := newFixture(t)
	f.tool("uname", "#!/bin/sh\necho Darwin\n")
	f.tool("sysctl", macSysctl)
	f.tool("pmset", "#!/bin/sh\nprintf \"Now drawing from 'Battery Power'\\n -InternalBattery-0 (id=4653155)\\t85%%; discharging; 4:10 remaining present: true\\n\"\n")
	f.tool("iostat", "#!/bin/sh\n[ \"$*\" = '-n 0 -C -c 2 -w 1' ] && printf '      cpu    load average\\n us sy id   1m   5m   15m\\n 18  5 77  2.36 1.85 1.87\\n  6  5 89  2.36 1.85 1.87\\n'\n")
	f.tool("notifyutil", "#!/bin/sh\n[ \"$*\" = '-g com.apple.system.thermalpressurelevel' ] && echo 'com.apple.system.thermalpressurelevel 2'\n")
	f.tool("route", "#!/bin/sh\n[ \"$*\" = '-n get default' ] && printf '   route to: default\\ndestination: default\\n  interface: en0\\n'\n")
	f.tool("netstat", `#!/bin/sh
[ "$*" = -ibn ] && printf '%s\n' \
'Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll' \
'lo0        16384 <Link#1>                          9183     0    1180112     9183     0    1180112     0' \
'en0        1500  <Link#14>   72:ae:25:19:37:bb 32971209     0 39134580381 12891787     0 6549722863     0' \
'en0        1500  192.168.0     192.168.0.48    32971209     - 39134580381 12891787     - 6549722863     -'
`)
	w := newWatcher("mac", "", nil, log.New(io.Discard, "", 0), "")
	stats := firstStats(t, f.machine())
	w.hear(stats, time.Now())
	live := readLive(t, "mac")
	if live.Pressure != "warning" || live.Swap != (space{UsedMB: 1095, TotalMB: 2048}) || !live.BootedAt.Equal(time.Unix(1794928938, 0)) {
		t.Errorf("pressure %q, swap %+v, booted at %s; want warning, 1095 of 2048 MB, %s", live.Pressure, live.Swap, live.BootedAt, time.Unix(1794928938, 0))
	}
	if live.Battery == nil || *live.Battery != (battery{Percent: 85, OnBattery: true}) {
		t.Errorf("battery = %+v, want 85%%, on battery", live.Battery)
	}
	if last := live.History[len(live.History)-1]; last.CPU == nil || *last.CPU != 11 || last.Thermal == nil || *last.Thermal != 2 || !strings.Contains(stats, " net=39134580381:6549722863:en0 ") {
		t.Errorf("stats line %q, want the second iostat sample's 11%% busy, thermal pressure 2 and en0's link counters", stats)
	}
}

func TestTheWatcherTurnsCountersIntoRates(t *testing.T) {
	newFixture(t)
	w := newWatcher("server", "", nil, log.New(io.Discard, "", 0), "")
	start := time.Unix(1_800_000_000, 0)
	for i, fields := range []string{
		"ticks=1000:4000 net=1000:500:eth0 temp=54.3:105",
		"ticks=1300:5000 net=31000:3500:eth0 temp=61.0",
		"ticks=10:100 net=91000:6500:wlan0",
		"net=121000:6500:wlan0 cpu=17 thermal=1",
	} {
		w.hear("stats 10 1 100 200 300 400 0 0 1794928938 "+fields, start.Add(time.Duration(i)*30*time.Second))
		if i == 0 && (w.live.Critical == nil || *w.live.Critical != 105) {
			t.Errorf("critical = %v, want 105 °C", w.live.Critical)
		}
	}
	live := readLive(t, "server")
	got := []string{}
	for _, r := range live.History {
		got = append(got, strings.Join([]string{value(r.CPU), value(r.Temp), value(r.Thermal), value(r.Received), value(r.Sent)}, " "))
	}
	want := []string{"- 54.3 - - -", "30 61 - 1000 100", "- - - - -", "17 - 1 1000 0"}
	if fmt.Sprint(got) != fmt.Sprint(want) || live.Critical != nil {
		t.Errorf("readings %q with critical %v, want %q and no critical once the line stops saying it", got, live.Critical, want)
	}
}

func TestAStatsLineThatBreaksPartWayChangesNothing(t *testing.T) {
	newFixture(t)
	w := newWatcher("server", "", nil, log.New(io.Discard, "", 0), "")
	start := time.Unix(1_800_000_000, 0)
	w.hear("stats 10 1 100 200 300 400 0 0 1794928938 pressure=normal ticks=1000:4000 net=1000:500:eth0 nic=1000:500:eth0 temp=54.3:105", start)
	live, ticks, traffic, nics := w.live, w.ticks, w.traffic, w.nics
	w.hear("stats 12 3 150 200 350 400 0 0 1794928999 pressure=critical battery=40:true ticks=1300:5000 net=31000:3500:eth0 nic=31000:3500:eth0 proc=7:1:2:0:sh part=app:1 vol=1:2:/:root sensor=50.0::cpu temp=61.0:99 thermal=x", start.Add(30*time.Second))
	if !reflect.DeepEqual(w.live, live) || w.ticks != ticks || w.traffic != traffic || !reflect.DeepEqual(w.nics, nics) {
		t.Errorf("after an unreadable line the watcher holds %+v, %+v, %+v, %+v; want %+v, %+v, %+v, %+v", w.live, w.ticks, w.traffic, w.nics, live, ticks, traffic, nics)
	}
}

func value[T any](p *T) string {
	if p == nil {
		return "-"
	}
	return fmt.Sprint(*p)
}

func TestALinuxMachineReportsItsMemoryPressureSwapBootTimeBatteryAndCounters(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("reads /proc/loadavg")
	}
	f := newFixture(t)
	f.tool("cat", `#!/bin/sh
[ "$1" = /proc/meminfo ] || exec /bin/cat "$@"
printf 'MemTotal: 16000000 kB\nMemAvailable: 8000000 kB\nSwapTotal: 2097152 kB\nSwapFree: 1048576 kB\nbtime 1794928938\n'
`)
	f.tool("grep", `#!/bin/sh
case $* in
*/proc/pressure/memory) echo 'some avg10=45.00 avg60=12.50 avg300=3.00 total=1' ;;
*) printf '%s\n' /sys/class/power_supply/AC/type:Mains /sys/class/power_supply/BAT0/type:Battery /sys/class/power_supply/BAT0/capacity:64 /sys/class/power_supply/BAT0/status:Discharging /sys/class/power_supply/hidpp_battery_0/type:Battery /sys/class/power_supply/hidpp_battery_0/scope:Device /sys/class/power_supply/hidpp_battery_0/capacity:10 /sys/class/power_supply/hidpp_battery_0/status:Charging ;;
esac
`)
	w := newWatcher("linux", "", nil, log.New(io.Discard, "", 0), "")
	stats := firstStats(t, f.machine())
	w.hear(stats, time.Now())
	if !regexp.MustCompile(` ticks=\d+:\d+( temp=\d+\.\d(:\d+)?)? net=\d+:\d+:\S+$`).MatchString(stats) {
		t.Errorf("stats line %q, want CPU ticks, the temperature where a sensor is known and the default route's byte counters", stats)
	}
	live := readLive(t, "linux")
	if live.Pressure != "warning" || live.Swap != (space{UsedMB: 1024, TotalMB: 2048}) || !live.BootedAt.Equal(time.Unix(1794928938, 0)) {
		t.Errorf("pressure %q, swap %+v, booted at %s; want warning from avg60, 1024 of 2048 MB, %s", live.Pressure, live.Swap, live.BootedAt, time.Unix(1794928938, 0))
	}
	if live.Battery == nil || *live.Battery != (battery{Percent: 64, OnBattery: true}) {
		t.Errorf("battery = %+v, want the system battery at 64%% and discharging", live.Battery)
	}
}

func hourOfStats(w *watcher, start time.Time) {
	for i, at := range []time.Duration{0, 30 * time.Second, time.Hour, time.Hour + 30*time.Second} {
		w.hear(fmt.Sprintf("stats 10 %d 100 200 300 400 0 0 1794928938 42:%d:150 cpu=%d", i, 3*i, 10*i), start.Add(at))
	}
}

func TestTheLiveFileKeepsTheLastHourOfMachineAndSessionCPU(t *testing.T) {
	newFixture(t)
	w := newWatcher("server", "", nil, log.New(io.Discard, "", 0), "")
	w.live.Sessions = []liveEntry{{ID: "a", pid: 42}}
	start := time.Unix(1_800_000_000, 0)
	hourOfStats(w, start)
	live := readLive(t, "server")
	from := start.Add(30 * time.Second).Unix()
	want := []reading{{At: from, CPU: new(10)}, {At: from + 3570, CPU: new(20)}, {At: from + 3600, CPU: new(30)}}
	if !reflect.DeepEqual(live.History, want) {
		t.Errorf("history = %+v, want the readings of the last hour %+v", live.History, want)
	}
	wantCPU := []cpuReading{{At: from, CPU: 10}, {At: from + 3570, CPU: 0}, {At: from + 3600, CPU: 10}}
	if fmt.Sprint(live.Sessions[0].CPUHistory) != fmt.Sprint(wantCPU) {
		t.Errorf("session CPU history = %+v, want %+v", live.Sessions[0].CPUHistory, wantCPU)
	}
}

func TestARestartedWatcherKeepsWhatTheLiveFileSaysUntilItHearsStats(t *testing.T) {
	f := newFixture(t)
	f.serverHome()
	w := newWatcher("server", "", nil, log.New(io.Discard, "", 0), "")
	w.live.Sessions = []liveEntry{{ID: "a", pid: 42}}
	hourOfStats(w, time.Unix(1_800_000_000, 0))
	kept := readLive(t, "server")
	restarted := newWatcher("server", "me@server", f.remoteHost(), log.New(io.Discard, "", 0), "")
	restarted.hear("sessions", time.Now())
	if live := readLive(t, "server"); !reflect.DeepEqual(live, kept) {
		t.Errorf("before its first stats a restarted watcher wrote %+v", live)
	}
	f.write(f.home, "ssh-down", "")
	restarted.follow("")
	kept.SSH, kept.Answering, kept.Sessions = "me@server", false, []liveEntry{}
	if live := readLive(t, "server"); !reflect.DeepEqual(live, kept) || fmt.Sprint(restarted.cpu) != fmt.Sprint(w.cpu) {
		t.Errorf("a restarted watcher that never connected wrote %+v with CPU %+v, want %+v with %+v", live, restarted.cpu, kept, w.cpu)
	}
}

func TestASessionInTwoProcessesIsOneEntryWithTheLatestState(t *testing.T) {
	newFixture(t)
	w := newWatcher("server", "", nil, log.New(io.Discard, "", 0), "")
	id := "0f0f0f0f-1111-2222-3333-555555555561"
	start := time.Unix(1_800_000_000, 0)
	for _, line := range []string{
		`claude {"pid":42,"sessionId":"` + id + `","cwd":"/srv/api","status":"busy","statusUpdatedAt":2000}`,
		`claude {"pid":41,"sessionId":"` + id + `","cwd":"/srv/api","status":"idle","statusUpdatedAt":1000}`,
		"sessions",
		"stats 10 1 100 200 300 400 0 0 1794928938 41:1:100 42:1:200",
	} {
		w.hear(line, start)
	}
	w.hear("stats 10 1 100 200 300 400 0 0 1794928938 41:31:100 42:4:200", start.Add(30*time.Second))
	live := readLive(t, "server")
	if len(live.Sessions) != 1 || live.Sessions[0].State != "busy" || live.Sessions[0].MemoryMB != 200 || fmt.Sprint(live.Sessions[0].CPUHistory) != fmt.Sprint([]cpuReading{{At: start.Unix() + 30, CPU: 10}}) {
		t.Errorf("live sessions = %+v, want one busy entry with only its own process's CPU", live.Sessions)
	}
}

func TestAMachineReachedThroughATailscaleRelayIsMarkedRelayed(t *testing.T) {
	f := newFixture(t)
	t.Cleanup(func() { tailnet.peers = nil })
	f.tool("tailscale", `#!/bin/sh
[ "$*" = "status --json" ] && echo '{"Peer":{
"nodekey:1":{"HostName":"homelab","DNSName":"homelab.tail1.ts.net.","TailscaleIPs":["100.64.0.1"],"CurAddr":"","Relay":"fra","Online":true},
"nodekey:2":{"HostName":"Studio","DNSName":"studio.tail1.ts.net.","TailscaleIPs":["100.64.0.2"],"CurAddr":"192.168.1.20:41641","Relay":"fra","Online":true},
"nodekey:3":{"HostName":"pi","DNSName":"pi.tail1.ts.net.","TailscaleIPs":["100.64.0.3"],"CurAddr":"","Relay":"fra","Online":false}}}'
`)
	var logged strings.Builder
	logger := log.New(&logged, "", 0)
	readTailnet(logger)
	for ssh, want := range map[string]bool{"me@homelab": true, "homelab.tail1.ts.net": true, "100.64.0.1": true, "studio": false, "Studio": false, "pi": false, "elsewhere": false} {
		w := newWatcher(ssh, ssh, nil, logger, "")
		w.hear("stats 10 1.5 100 200 300 400 0 0 1794928938", time.Now())
		if got := readLive(t, ssh).Relayed; got != want {
			t.Errorf("%s reads as relayed %t, want %t", ssh, got, want)
		}
	}
	f.tool("tailscale", "#!/bin/sh\necho '{\"Peer\":'\n")
	readTailnet(logger)
	if relayed("homelab") || !strings.Contains(logged.String(), "tailscale's status is unreadable") {
		t.Errorf("after unreadable JSON homelab reads as relayed %t and the log says %q", relayed("homelab"), logged.String())
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
	started := filepath.Join(t.TempDir(), "pid")
	out, err := exec.Command("tmux", "new-session", "-d", "-P", "-F", "#{pane_id}", `sh -c 'sleep 60 & echo $! > "$0"; wait' `+started+`; :`).Output()
	if err != nil {
		t.Fatal(err)
	}
	pane := strings.TrimSpace(string(out))
	if !waitForFile(t, started, 10*time.Second) {
		t.Fatal("the session process never started")
	}
	data, _ := os.ReadFile(started)
	pid := strings.TrimSpace(string(data))
	f.write(filepath.Join(server, ".claude", "sessions"), pid+".json", `{"pid":`+pid+`,"sessionId":"0f0f0f0f-1111-2222-3333-555555555559","cwd":"/srv/api","status":"idle","bridgeSessionId":"session_42"}`)
	go newWatcher("server", "me@server", f.remoteHost(), log.New(io.Discard, "", 0), "").follow("")
	for deadline := time.Now().Add(10 * time.Second); ; time.Sleep(100 * time.Millisecond) {
		if live := readLive(t, "server"); len(live.Sessions) == 1 {
			if s := live.Sessions[0]; s.RemoteControl != "https://claude.ai/code/session_42" || s.Tmux != pane || live.SSH != "me@server" {
				t.Errorf("a session two processes below its pane opens through %q and tmux pane %q (want %s), the machine over ssh %q", s.RemoteControl, s.Tmux, pane, live.SSH)
			}
			return
		}
		if time.Now().After(deadline) {
			t.Fatal("the session never reached the live file")
		}
	}
}

func sessionsOnce(t *testing.T, m sh.Machine, w *watcher) {
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
		w.hear(lines.Text(), time.Now())
		if lines.Text() == "sessions" {
			w.hear("stats 10 1 100 200 300 400 0 0 1794928938", time.Now())
			return
		}
	}
	t.Fatal("the watcher never reported its sessions")
}

func (f *fixture) sleeper(env ...string) int {
	f.t.Helper()
	cmd := exec.Command("sleep", "60")
	cmd.Env = append(os.Environ(), env...)
	if err := cmd.Start(); err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { cmd.Process.Kill(); cmd.Wait() })
	return cmd.Process.Pid
}

func TestAMacSessionSaysWhichAppAndTerminalItRunsIn(t *testing.T) {
	f := newFixture(t)
	desktop, ghostty, tmux, alone, linked := f.sleeper(), f.sleeper(), f.sleeper(), f.sleeper(), f.sleeper()
	sessions := filepath.Join(f.home, ".claude", "sessions")
	for pid, fields := range map[int]string{
		desktop: `"entrypoint":"claude-desktop","hostSessionId":"local_7cbf"`,
		ghostty: `"entrypoint":"cli","hostSessionId":"local_none"`,
		tmux:    `"entrypoint":"cli"`,
		alone:   `"entrypoint":"cli"`,
		linked:  `"entrypoint":"cli"`,
	} {
		f.write(sessions, fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"0f0f0f0f-1111-2222-3333-%012d","cwd":"/srv","status":"idle",%s}`, pid, pid, fields))
	}
	f.write(f.home, "ps-table", fmt.Sprintf("%d 900 ??\n900 901 ??\n901 1 ??\n%d 910 ttys004\n910 911 ttys004\n911 912 ttys004\n912 1 ??\n%d 950 ttys009\n950 951 ttys009\n951 1 ??\n%d 955 ttys010\n955 1 ??\n%d 957 ttys011\n957 1 ??\n960 961 ttys007\n961 912 ttys007\n970 971 ttys008\n971 1 ??\n", desktop, ghostty, tmux, alone, linked))
	f.write(f.home, "ps-comm", "900 /Applications/Claude.app/Contents/Helpers/disclaimer\n901 /Applications/Claude.app/Contents/MacOS/Claude\n910 -zsh\n911 login\n912 /Users/sam/Sam's Apps/Ghostty.app/Contents/MacOS/ghostty\n960 tmux\n961 login\n970 tmux\n971 /System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal\n")
	f.tool("uname", "#!/bin/sh\necho Darwin\n")
	f.tool("ps", `#!/bin/sh
case "$*" in
"-A -o pid= -o ppid= -o tty=") cat "$HOME/ps-table" ;;
"-o pid= -o comm= -p "*) awk -v want=",$6," 'index(want, "," $1 ",")' "$HOME/ps-comm" ;;
esac
`)
	f.tool("tmux", "#!/bin/sh\nprintf '%s\\n' 'pane 950 %4 $1' 'pane 955 %5 $3' 'pane 957 %6 $1' 'pane 957 %6 $2' 'client 1700000100 960 /dev/ttys007 $2' 'client 1700000000 970 /dev/ttys008 $1'\n")
	f.tool("plutil", `#!/bin/sh
case "$*" in
"-extract CFBundleIdentifier raw -o - /Applications/Claude.app/Contents/Info.plist") echo com.anthropic.claudefordesktop ;;
"-extract CFBundleIdentifier raw -o - /Users/sam/Sam's Apps/Ghostty.app/Contents/Info.plist") echo com.mitchellh.ghostty ;;
"-extract CFBundleIdentifier raw -o - /System/Applications/Utilities/Terminal.app/Contents/Info.plist") echo com.apple.Terminal ;;
esac
`)
	w := newWatcher("mac", "", nil, log.New(io.Discard, "", 0), "")
	sessionsOnce(t, f.machine(), w)
	got := map[int]string{}
	for _, s := range readLive(t, "mac").Sessions {
		var pid int
		fmt.Sscanf(s.ID[len(s.ID)-12:], "%d", &pid)
		got[pid] = fmt.Sprintf("%s|%s|%s|%s|%v", s.HostSession, s.Tmux, s.TTY, s.App, s.Rex)
	}
	want := map[int]string{
		desktop: "local_7cbf|||com.anthropic.claudefordesktop|<nil>",
		ghostty: "||/dev/ttys004|com.mitchellh.ghostty|<nil>",
		tmux:    "|%4|/dev/ttys008|com.apple.Terminal|<nil>",
		alone:   "|%5|||<nil>",
		linked:  "|%6|/dev/ttys007|com.mitchellh.ghostty|<nil>",
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("sessions run in %v, want %v", got, want)
	}
}

func TestALinuxSessionInRexSaysItsSessionAndBlock(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("only Linux shows other processes their environment")
	}
	f := newFixture(t)
	pid := f.sleeper("REX_SESSION=session:01a1", "REX_BLOCK=block:01a2")
	f.write(filepath.Join(f.home, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"0f0f0f0f-1111-2222-3333-555555555562","cwd":"/srv","status":"idle"}`, pid))
	w := newWatcher("box", "", nil, log.New(io.Discard, "", 0), "")
	sessionsOnce(t, f.machine(), w)
	if s := readLive(t, "box").Sessions; len(s) != 1 || s[0].Rex == nil || *s[0].Rex != (rex{Session: "session:01a1", Block: "block:01a2"}) || s[0].App != "" || s[0].TTY != "" {
		t.Errorf("live sessions = %+v, want the Rex session and block from its environment", s)
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

func (f *fixture) session(pid int, id string) {
	f.t.Helper()
	f.write(filepath.Join(f.home, ".claude", "sessions"), fmt.Sprint(pid, ".json"), fmt.Sprintf(`{"pid":%d,"sessionId":"%s","cwd":"/srv","status":"idle"}`, pid, id))
}

func TestAMacListsItsBusiestProcessesMemoryPartsVolumesAndInterfaces(t *testing.T) {
	f := newFixture(t)
	pid := f.sleeper()
	id := "0f0f0f0f-1111-2222-3333-555555555563"
	f.session(pid, id)
	f.write(f.home, "ps-1", testdata(t, "darwin/ps-1.txt")+fmt.Sprintf("%d 1 0:01.00 0:01.00 2048 claude\n99001 %d 0:30.00 0:30.00 1024 ssh\n", pid, pid))
	f.write(f.home, "ps-2", testdata(t, "darwin/ps-2.txt")+fmt.Sprintf("%d 1 0:01.50 0:01.50 2048 claude\n99001 %d 0:38.00 0:38.00 1024 ssh\n", pid, pid))
	f.write(f.home, "comm", testdata(t, "darwin/comm.txt")+fmt.Sprintf("99001 ssh: /Users/sam/.magi/ssh/99e025c35b39f398887e16f50e350caa0e0c34a6 [mux]\n%d /Users/sam/Library/Application Support/Claude/claude-code/2.1.330/4262a1273231/claude.app/Contents/MacOS/claude\n", pid))
	f.tool("ps", `#!/bin/sh
case "$*" in
"-A -o pid= -o ppid= -o time= -o time= -o rss= -o ucomm=") if [ -f "$HOME/ps-1" ]; then /bin/cat "$HOME/ps-1"; rm "$HOME/ps-1"; else /bin/cat "$HOME/ps-2"; fi ;;
"-o pid= -o comm= -p "*) awk -v want=",$6," 'index(want, "," $1 ",")' "$HOME/comm" ;;
esac
`)
	f.tool("uname", "#!/bin/sh\necho Darwin\n")
	f.tool("sleep", "#!/bin/sh\n")
	f.tool("tmux", "#!/bin/sh\n")
	f.tool("sysctl", macSysctl)
	f.tool("iostat", "#!/bin/sh\nprintf '      cpu    load average\\n us sy id   1m   5m   15m\\n 18  5 77  2.36 1.85 1.87\\n  6  5 89  2.36 1.85 1.87\\n'\n")
	f.tool("route", "#!/bin/sh\n")
	f.served("netstat", "darwin/netstat.txt")
	f.served("vm_stat", "darwin/vm_stat.txt")
	f.served("mount", "darwin/mount.txt")
	f.served("diskutil", "darwin/diskutil.txt")
	f.df("darwin/df.txt")
	w := newWatcher("mac", "", nil, log.New(io.Discard, "", 0), "")
	stats := heardStats(t, f.machine(), w, 2, 10*time.Second)
	if !strings.Contains(stats[1], " nic=19650959258:1647554020:en0 nic=16985283:2210815:awdl0 nic=0:7500:nan0 nic=17711752:18086155:utun4 ") || strings.Contains(stats[1], "lo0") || strings.Contains(stats[1], ":gif0") {
		t.Errorf("stats line %q, want the byte counters of every interface with traffic but loopback", stats[1])
	}
	want := []string{
		fmt.Sprintf("cpu 99001 ssh 80%% 1MB %s", id), "cpu 374 mds 54% 44MB ", "cpu 417 WindowServer 11% 85MB ", "cpu 35178 Claude Helper 8% 66MB ",
		fmt.Sprintf("cpu %d claude 5%% 2MB %s", pid, id), "cpu 35197 Claude Helper (Renderer) 4% 632MB ", "cpu 35301 claude 4% 382MB ", "cpu 308 NotificationCenter 3% 59MB ",
		"memory 35197 Claude Helper (Renderer) 4% 632MB ", "memory 47883 Dia 1% 421MB ", "memory 35301 claude 4% 382MB ", "memory 4836 claude 0% 317MB ",
		"memory 35175 Claude 0% 305MB ", "memory 66745 Browser Helper (Aperitif Renderer) 0% 293MB ", "memory 65068 WhatsApp 2% 235MB ", "memory 61804 Browser Helper (Aperitif Renderer) 0% 215MB ",
		"part app 4527", "part wired 2331", "part compressed 4773", "part cached 3361", "part free 759",
		"volume Macintosh HD / 297762/471482", "volume Test Drive /Volumes/Test Drive 1/40",
	}
	if got := listed(readLive(t, "mac")); !reflect.DeepEqual(got, want) {
		t.Errorf("the Mac lists\n%q\nwant\n%q", got, want)
	}
}

func TestALinuxMachineListsItsBusiestProcessesMemoryPartsVolumesInterfacesAndSensors(t *testing.T) {
	f := newFixture(t)
	pid := f.sleeper()
	id := "0f0f0f0f-1111-2222-3333-555555555564"
	f.session(pid, id)
	for _, n := range []string{"1", "2"} {
		f.write(f.home, "stat-"+n, strings.ReplaceAll(testdata(t, "linux/stat-"+n+".txt"), "SESSION", fmt.Sprint(pid)))
	}
	data, err := filepath.Abs("testdata/linux")
	if err != nil {
		t.Fatal(err)
	}
	f.tool("cat", `#!/bin/sh
case "$*" in
"/proc/meminfo /proc/stat") /bin/cat '`+data+`/meminfo.txt'; echo 'btime 1794928938' ;;
"/proc/meminfo /proc/net/dev") /bin/cat '`+data+`/meminfo.txt' '`+data+`/net-dev.txt' ;;
/proc/mounts) /bin/cat '`+data+`/mounts.txt' ;;
/proc/*) if [ -f "$HOME/stat-1" ]; then /bin/cat "$HOME/stat-1"; rm "$HOME/stat-1"; else /bin/cat "$HOME/stat-2"; fi ;;
*) exec /bin/cat "$@" ;;
esac
`)
	f.tool("grep", "#!/bin/sh\ncase \"$*\" in *hwmon*) /bin/cat '"+data+"/hwmon.txt' ;; esac\n")
	f.tool("uname", "#!/bin/sh\necho Linux\n")
	f.tool("getconf", "#!/bin/sh\ncase $1 in CLK_TCK) echo 100 ;; PAGESIZE) echo 4096 ;; *) echo 8 ;; esac\n")
	f.tool("cut", "#!/bin/sh\necho '0.52 0.58 0.59'\n")
	f.tool("sleep", "#!/bin/sh\n")
	f.tool("tmux", "#!/bin/sh\n")
	f.tool("ps", "#!/bin/sh\n")
	f.df("linux/df.txt")
	w := newWatcher("linux", "", nil, log.New(io.Discard, "", 0), "")
	stats := heardStats(t, f.machine(), w, 2, 30*time.Second)
	if !strings.Contains(stats[1], " nic=98234556123:4523456789:enp3s0 nic=1234567:2345678:docker0 nic=5678901:6789012:tailscale0 ") || strings.Contains(stats[1], ":lo ") || strings.Contains(stats[1], "veth") || strings.Contains(stats[1], "wlp2s0") {
		t.Errorf("stats line %q, want the byte counters of every interface with traffic but loopback and veth", stats[1])
	}
	want := []string{
		fmt.Sprintf("cpu 4100 node 53%% 195MB %s", id), fmt.Sprintf("cpu 9999 cc1 40%% 78MB %s", id), "cpu 2301 Isolated Web Co 33% 469MB ", "cpu 6000 rsync 30% 2MB ",
		"cpu 2280 firefox-esr 25% 703MB ", fmt.Sprintf("cpu %d claude 23%% 391MB %s", pid, id), "cpu 2210 tmux: server 4% 6MB ", "cpu 5100 containerd 1% 59MB ",
		"memory 2280 firefox-esr 25% 703MB ", "memory 2301 Isolated Web Co 33% 469MB ", fmt.Sprintf("memory %d claude 23%% 391MB %s", pid, id), fmt.Sprintf("memory 4100 node 53%% 195MB %s", id),
		"memory 3005 postgres 0% 117MB ", "memory 5000 dockerd 0% 98MB ", fmt.Sprintf("memory 9999 cc1 40%% 78MB %s", id), "memory 5100 containerd 1% 59MB ",
		"part app 4326", "part cached 10704", "part free 891",
		"volume /mnt/media library /mnt/media library 693390/1876685", "volume / / 45224/96189", "volume /boot /boot 394/1946", "volume /boot/efi /boot/efi 6/1073",
		"sensor acpitz 53 119", "sensor Package id 0 51 100", "sensor Core 4 50 100", "sensor Core 12 49 100",
		"sensor Core 0 48 100", "sensor Core 8 47 100", "sensor Sensor 2 42.8 -", "sensor Composite 38.8 84",
	}
	if got := listed(readLive(t, "linux")); !reflect.DeepEqual(got, want) {
		t.Errorf("the Linux machine lists\n%q\nwant\n%q", got, want)
	}
}
