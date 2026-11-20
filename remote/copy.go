package main

import (
	"context"
	"fmt"
	"slices"
	"strconv"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

func describeCopy(m sh.Machine, hostName string, host Host, ws Workspace, here, codex bool) (string, error) {
	lines := []string{"What goes to " + hostName + ":"}
	secrets := nested(ws, func(w Workspace) []string { return w.Secrets })
	closing := "Nothing that's running goes."
	if ws.Git == nil {
		var err error
		if secrets, err = findSecrets(m, ws.Root, host.Secrets, host.Exclude, nil); err != nil {
			return "", err
		}
		out, err := folder(m, "size", ws.Root, pruneArgs(host.Exclude)...)
		if err != nil {
			return "", err
		}
		var files int
		var size int64
		if _, err := fmt.Sscan(string(out), &files, &size); err != nil {
			return "", fmt.Errorf("unexpected folder size %q from %s", out, m.Name())
		}
		line := fmt.Sprintf("  the folder, %d %s (%s)", files, plural(files, "file"), humanSize(size))
		if len(host.Exclude) > 0 {
			line += ", without " + names(host.Exclude)
		}
		lines = append(lines, line)
	} else {
		branch := "branch " + ws.Git.Branch
		if ws.Git.Branch == "" {
			branch = "the detached HEAD"
		}
		n := changed(ws)
		lines = append(lines, fmt.Sprintf("  %s, its commits, and %d %s", branch, n, plural(n, "changed file")))
		if untracked := nested(ws, func(w Workspace) []string { return w.Untracked }); len(untracked) > 0 {
			lines = append(lines, "  files git doesn't track yet: "+names(untracked))
		}
		closing = "Nothing git ignores goes, and nothing that's running."
		if len(secrets) > 0 {
			closing = "Nothing git ignores goes, apart from those secret files, and nothing that's running."
		}
	}
	if len(secrets) > 0 {
		lines = append(lines, "  secret files, by name only: "+names(secrets))
	}
	if n := len(nested(ws, func(w Workspace) []string { return w.LFSObjects })); n > 0 {
		lines = append(lines, fmt.Sprintf("  %d %s through Git LFS", n, plural(n, "file")))
	}
	if here {
		switch project, approved, err := readApproved(ws.Root); {
		case err != nil:
			lines = append(lines, "  "+err.Error())
		case project != nil:
			var parts []string
			if project.Setup != "" {
				parts = append(parts, "setup")
			}
			if n := len(project.Services); n > 0 {
				parts = append(parts, plural(n, "service")+" "+names(project.Services))
			}
			if n := len(project.Ports); n > 0 {
				ports := make([]string, n)
				for i, port := range project.Ports {
					ports[i] = strconv.Itoa(port)
				}
				parts = append(parts, plural(n, "port")+" "+names(ports))
			}
			line := "  from " + projectFile + ": " + strings.Join(parts, ", ")
			if !approved {
				line += " (not allowed yet, run magi allow)"
			}
			lines = append(lines, line)
		}
	}
	if codex {
		lines = append(lines, "  the conversation")
	} else {
		lines = append(lines, "  the conversation and its project memory")
	}
	return strings.Join(append(lines, closing), "\n"), nil
}

func describeHere(hostName string, host Host, sessionID string) (string, error) {
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return "", err
	}
	s, err := findSession(local, sessionID)
	if err != nil {
		return "", err
	}
	ws, _, err := sourceWorkspace(local, local.Name(), s.Cwd, host.Secrets, host.Exclude)
	if err != nil {
		return "", err
	}
	return describeCopy(local, hostName, host, ws, true, s.Codex)
}

func nested(ws Workspace, list func(Workspace) []string) []string {
	items := slices.Clone(list(ws))
	for _, sub := range ws.Submodules {
		for _, p := range nested(sub, list) {
			items = append(items, sub.Subpath+"/"+p)
		}
	}
	return items
}

func changed(ws Workspace) int {
	n := ws.Changed
	for _, sub := range ws.Submodules {
		n += changed(sub)
	}
	return n
}

func names(items []string) string {
	if len(items) > 5 {
		items = append(items[:5:5], fmt.Sprintf("%d more", len(items)-5))
	}
	if len(items) < 2 {
		return strings.Join(items, "")
	}
	return strings.Join(items[:len(items)-1], ", ") + " and " + items[len(items)-1]
}

func plural(n int, word string) string {
	if n == 1 {
		return word
	}
	return word + "s"
}

func humanSize(n int64) string {
	size, units := float64(n), []string{"bytes", "KB", "MB", "GB", "TB"}
	i := 0
	for ; size >= 1000 && i < len(units)-1; i++ {
		size /= 1000
	}
	return strconv.FormatFloat(size, 'f', min(i, 1), 64) + " " + units[i]
}
