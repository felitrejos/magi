package main

import (
	"context"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func machineFor(name, os, arch string, cpu float64, tools ...string) fleetMachine {
	return fleetMachine{Name: name, Live: true, OS: os, Arch: arch, CPU: cpu, Cores: 8, Tools: tools}
}

func TestALinuxBuildGoesToTheLinuxMachineBeforeAContainerOnTheMac(t *testing.T) {
	homelab := machineFor("homelab", "Ubuntu 26.04", "x86_64", 0.6, "cargo")
	studio := machineFor("studio", "macOS 27.2", "aarch64", 0.1, "docker")
	mac := machineFor("mac", "macOS 27.2", "aarch64", 0.05, "docker", "cargo")
	view := fleetView{
		Machines: []machineInfo{{Name: "homelab", Fleet: &homelab}, {Name: "studio", Fleet: &studio}},
		Here:     &mac,
	}
	needs := needsInput{OS: "linux", Arch: "amd64"}
	places := view.places(needs)
	if len(places) != 3 || places[0].name != "homelab" || !places[0].native {
		t.Fatalf("a native Linux machine comes first, however busy: %v", places)
	}
	if places[1].name != "mac" || places[2].name != "studio" {
		t.Fatalf("then containers, lightest first: %v", places)
	}
	text := describePlaces(view, needs)
	if !strings.Contains(text, "Suggested: homelab, natively (Ubuntu 26.04 x86_64)") {
		t.Fatal(text)
	}
}

func TestWorkThatNeedsTheMacStaysOnAMacAndAMissingToolRulesAMachineOut(t *testing.T) {
	homelab := machineFor("homelab", "Ubuntu 26.04", "x86_64", 0.0, "docker")
	mac := machineFor("mac", "macOS 27.2", "aarch64", 0.9, "xcodebuild")
	view := fleetView{Machines: []machineInfo{{Name: "homelab", Fleet: &homelab}}, Here: &mac}
	places := view.places(needsInput{OS: "macos", Tools: []string{"xcodebuild"}})
	if len(places) != 1 || !places[0].here {
		t.Fatalf("only this Mac can build with Xcode, and no container can stand in: %v", places)
	}
	if text := describePlaces(view, needsInput{Tools: []string{"nvidia-smi"}}); !strings.Contains(text, "No machine can run") {
		t.Fatal(text)
	}
}

func TestAMachineWithoutRoomOrOutOfReachIsNotOffered(t *testing.T) {
	homelab := machineFor("homelab", "Ubuntu 26.04", "x86_64", 0.0)
	view := fleetView{Machines: []machineInfo{{Name: "homelab", Fleet: &homelab, NoRoom: "2 tasks already"}}}
	if places := view.places(needsInput{OS: "linux"}); len(places) != 0 {
		t.Fatalf("%v", places)
	}
}

func TestAToolMagiDoesntCheckForRulesNoMachineOut(t *testing.T) {
	homelab := machineFor("homelab", "Ubuntu 26.04", "x86_64", 0.2, "node")
	view := fleetView{Machines: []machineInfo{{Name: "homelab", Fleet: &homelab}}}
	needs := needsInput{OS: "linux", Tools: []string{"node", "pnpm"}}
	if places := view.places(needs); len(places) != 1 || !places[0].native {
		t.Fatalf("%v", places)
	}
	if text := describePlaces(view, needs); !strings.Contains(text, "pnpm isn't checked, so it isn't counted") || strings.Contains(text, "No machine can run") {
		t.Fatal(text)
	}
}

func TestAMachineMagiDoesntReportIsRankedFromWhatSSHFound(t *testing.T) {
	homelab := parsePlatform("homelab", "Ubuntu 26.04.1 LTS\nx86_64\npython3 make docker \n")
	if homelab.OS != "Ubuntu 26.04.1 LTS" || homelab.Arch != "x86_64" || strings.Join(homelab.Tools, ",") != "python3,make,docker" {
		t.Fatalf("%+v", homelab)
	}
	mac := machineFor("mac", "macOS 27.2", "arm64", 0.1, "docker", "python3")
	view := fleetView{Machines: []machineInfo{{Name: "homelab", Fleet: homelab}}, Here: &mac}
	needs := needsInput{OS: "linux", Tools: []string{"python3"}}
	places := view.places(needs)
	if len(places) != 2 || places[0].name != "homelab" || !places[0].native {
		t.Fatalf("the Linux machine still comes first when only SSH could say what it is: %v", places)
	}
	text := describePlaces(view, needs)
	if !strings.Contains(text, "homelab: natively (Ubuntu 26.04.1 LTS x86_64), load unknown") {
		t.Fatal(text)
	}
}

func TestAMachineWithKnownLoadComesBeforeOneWithUnknownLoad(t *testing.T) {
	known := machineFor("studio", "Ubuntu 26.04", "x86_64", 0.4)
	unknown := parsePlatform("homelab", "Ubuntu 26.04\nx86_64\n\n")
	view := fleetView{Machines: []machineInfo{{Name: "homelab", Fleet: unknown}, {Name: "studio", Fleet: &known}}}
	places := view.places(needsInput{OS: "linux"})
	if len(places) != 2 || places[0].name != "studio" {
		t.Fatalf("%v", places)
	}
}

func TestAMachineReadOverSSHWithoutMagiSaysMagiDoesntReportIt(t *testing.T) {
	if got := parsePlatform("homelab", "Ubuntu 26.04\nx86_64\n\n").describe(); !strings.HasPrefix(got, "the overview doesn't report it, so its load is unknown") {
		t.Fatal(got)
	}
}

func TestAMachineWithoutTheOverviewReportsItsLoadOverSSH(t *testing.T) {
	m := parsePlatform("homelab", "Ubuntu 26.04\nx86_64\ndocker go\n8\n3.20 2.10 1.00\n16000000000 4000000000\n2\n")
	if !m.measured() || m.Cores != 8 || m.Load[0] != 3.2 || m.MemoryUsed != 12e9 || m.Agents.Working != 2 {
		t.Fatalf("parsed %+v", m)
	}
	if got := m.describe(); got != "load 3.20 on 8 cores, memory 11.2 GB of 14.9 GB, 2 Claude sessions working, read over SSH" {
		t.Errorf("describe = %q", got)
	}
	if room := (Budget{MaxAgents: 2}).noRoom(time.Now(), m, true); !strings.Contains(room, "2 agents are working") {
		t.Errorf("noRoom = %q", room)
	}
	if partial := parsePlatform("old", "Ubuntu\nx86_64\n\n"); partial.measured() {
		t.Errorf("a machine with no load lines counts as measured: %+v", partial)
	}
	newFixture(t)
	local, err := newLocal(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	out, err := sh(local, platformScript, local.Claude())
	if err != nil {
		t.Fatal(err)
	}
	if here := parsePlatform("here", string(out)); !here.measured() || here.MemoryTotal <= 0 {
		t.Errorf("this machine's own probe = %+v from %q", here, out)
	}
}

const macSysctl = `#!/bin/sh
for name; do
	case $name in
	hw.ncpu) echo 10 ;;
	vm.loadavg) echo '{ 2.56 2.55 2.64 }' ;;
	hw.memsize) echo 17179869184 ;;
	hw.pagesize) echo 16384 ;;
	vm.page_free_count) echo 12520 ;;
	vm.page_pageable_external_count) echo 180728 ;;
	vm.page_purgeable_count) echo 5018 ;;
	esac
done
`

func TestAMacsAvailableMemoryIsItsFreeCachedAndPurgeablePages(t *testing.T) {
	f := newFixture(t)
	f.tool("uname", "#!/bin/sh\ncase $1 in -s) echo Darwin ;; -m) echo arm64 ;; esac\n")
	f.tool("sw_vers", "#!/bin/sh\necho 27.2\n")
	f.tool("sysctl", macSysctl)
	out, err := sh(f.machine(), platformScript, "claude")
	if err != nil {
		t.Fatal(err)
	}
	m := parsePlatform("mac", string(out))
	if m.OS != "macOS 27.2" || m.Cores != 10 || m.Load[0] != 2.56 {
		t.Fatalf("parsed %+v from %q", m, out)
	}
	if available := m.MemoryTotal - m.MemoryUsed; m.MemoryTotal != 16<<30 || available != (12520+180728+5018)*16384 {
		t.Errorf("total %.0f, available %.0f; want 16 GiB and the free, file-backed and purgeable pages", m.MemoryTotal, available)
	}
}

func TestATargetRechecksMemoryTheWayTheProbeReadsIt(t *testing.T) {
	f := newFixture(t)
	f.tool("uname", "#!/bin/sh\necho Darwin\n")
	f.tool("sysctl", macSysctl)
	_, err := sh(f.machine(), taskScript, "room", filepath.Join(f.home, ".magi", "tasks", "0000aaaa"), "delegate", "0", strconv.Itoa(4<<30))
	if err == nil || !strings.Contains(err.Error(), "3.0 GB of memory is free there, its budget needs 4.0 GB") {
		t.Errorf("err = %v, want the probe's free memory in the probe's units", err)
	}
}

func TestMemoryIsShownInTheUnitsBudgetsAreWrittenIn(t *testing.T) {
	m := parsePlatform("mac", "macOS 27.2\narm64\n\n10\n2.56 2.55 2.64\n17179869184 3221225472\n0\n")
	if total, err := parseMemory("16.0 GB"); err != nil || total != m.MemoryTotal {
		t.Errorf("the 16.0 GB shown parses back as %v, %v; the machine has %v", total, err, m.MemoryTotal)
	}
	tight := Budget{MinFree: 4 << 30}.noRoom(time.Now(), m, true)
	if tight != "3.0 GB of memory is free there, its budget needs 4.0 GB" {
		t.Errorf("noRoom = %q", tight)
	}
}
