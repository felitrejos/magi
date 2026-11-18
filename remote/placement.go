package main

import (
	"fmt"
	"slices"
	"sort"
	"strings"
)

type needsInput struct {
	OS    string   `json:"os,omitempty" jsonschema:"the operating system the work has to run on: linux or macos"`
	Arch  string   `json:"arch,omitempty" jsonschema:"the CPU architecture it has to run on: x86_64 or arm64"`
	Tools []string `json:"tools,omitempty" jsonschema:"programs it needs; only cargo, go, node, python3, swift, xcodebuild, gcc, clang, make, nvidia-smi, docker and podman are checked, and any other tool is left out of the choice"`
}

var (
	checkedTools   = []string{"cargo", "go", "node", "python3", "swift", "xcodebuild", "gcc", "clang", "make", "nvidia-smi", "docker", "podman"}
	containerTools = []string{"docker", "podman"}
	hostOnlyTools  = []string{"xcodebuild", "nvidia-smi"}
)

func (n needsInput) empty() bool {
	return n.OS == "" && n.Arch == "" && len(n.Tools) == 0
}

func (n needsInput) String() string {
	var parts []string
	for _, part := range []string{n.OS, n.Arch} {
		if part != "" {
			parts = append(parts, part)
		}
	}
	return strings.Join(append(parts, n.Tools...), ", ")
}

func sameOS(want, os string) bool {
	os = strings.ToLower(os)
	switch strings.ToLower(want) {
	case "macos", "mac", "darwin":
		return strings.Contains(os, "macos")
	case "linux":
		return !strings.Contains(os, "macos") && os != ""
	default:
		return strings.Contains(os, strings.ToLower(want))
	}
}

func sameArch(want, arch string) bool {
	canonical := func(arch string) string {
		switch strings.ToLower(arch) {
		case "amd64", "x64", "x86_64":
			return "x86_64"
		case "arm64", "aarch64":
			return "arm64"
		}
		return strings.ToLower(arch)
	}
	return canonical(want) == canonical(arch)
}

func (n needsInput) native(m fleetMachine) bool {
	if n.OS != "" && !sameOS(n.OS, m.OS) {
		return false
	}
	if n.Arch != "" && !sameArch(n.Arch, m.Arch) {
		return false
	}
	return n.hasTools(m)
}

func (n needsInput) hasTools(m fleetMachine) bool {
	for _, tool := range n.Tools {
		if slices.Contains(checkedTools, tool) && !slices.Contains(m.Tools, tool) {
			return false
		}
	}
	return true
}

func (n needsInput) container(m fleetMachine) (string, bool) {
	if n.OS != "" && !sameOS("linux", n.OS) {
		return "", false
	}
	for _, tool := range n.Tools {
		if slices.Contains(hostOnlyTools, tool) {
			return "", false
		}
	}
	for _, tool := range containerTools {
		if slices.Contains(m.Tools, tool) {
			return tool, true
		}
	}
	return "", false
}

type spot struct {
	name    string
	native  bool
	how     string
	busy    float64
	here    bool
	unknown bool
}

func (p spot) String() string {
	where := p.name
	if p.here {
		where = "this machine (" + p.name + ")"
	}
	if p.unknown {
		return fmt.Sprintf("%s: %s, load unknown", where, p.how)
	}
	return fmt.Sprintf("%s: %s, %.0f%% busy", where, p.how, p.busy*100)
}

func (view fleetView) places(needs needsInput) []spot {
	var places []spot
	consider := func(m fleetMachine, name string, here bool) {
		if !m.Live && !m.Probed {
			return
		}
		how := ""
		native := needs.native(m)
		switch {
		case native:
			how = "natively (" + platform(m) + ")"
		default:
			tool, ok := needs.container(m)
			if !ok {
				return
			}
			how = fmt.Sprintf("in a %s container, since it's %s", tool, platform(m))
		}
		places = append(places, spot{name: name, native: native, how: how, busy: m.busy(), here: here, unknown: !m.measured()})
	}
	for _, m := range view.Machines {
		if m.Fleet == nil || m.Reachable != nil || m.NoRoom != "" {
			continue
		}
		consider(*m.Fleet, m.Name, false)
	}
	if view.Here != nil {
		consider(*view.Here, view.Here.Name, true)
	}
	sort.SliceStable(places, func(i, j int) bool {
		if places[i].native != places[j].native {
			return places[i].native
		}
		if places[i].unknown != places[j].unknown {
			return places[j].unknown
		}
		return places[i].busy < places[j].busy
	})
	return places
}

func (n needsInput) unchecked() string {
	var b strings.Builder
	for _, tool := range n.Tools {
		if !slices.Contains(checkedTools, tool) {
			fmt.Fprintf(&b, "%s isn't checked, so it isn't counted.\n", tool)
		}
	}
	return b.String()
}

func describePlaces(view fleetView, needs needsInput) string {
	places := view.places(needs)
	if len(places) == 0 {
		return fmt.Sprintf("\nNo machine can run work that needs %s: none has it natively, and none with room has Docker or Podman running.\n", needs) + needs.unchecked()
	}
	var b strings.Builder
	fmt.Fprintf(&b, "\nFor work that needs %s, best first:\n", needs)
	b.WriteString(needs.unchecked())
	for _, p := range places {
		fmt.Fprintf(&b, "- %s\n", p)
	}
	best := places[0]
	if best.here {
		fmt.Fprintf(&b, "Suggested: run it here, %s.\n", best.how)
	} else {
		fmt.Fprintf(&b, "Suggested: %s, %s.\n", best.name, best.how)
	}
	return b.String()
}

func platform(m fleetMachine) string {
	return strings.Join(strings.Fields(m.OS+" "+m.Arch), " ")
}
