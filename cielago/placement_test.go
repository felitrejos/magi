package main

import (
	"strings"
	"testing"
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
