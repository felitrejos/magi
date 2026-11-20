package main

import (
	"fmt"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"github.com/felitrejos/magi/remote/internal/sh"
)

const askSize = 50_000_000

func describeCopy(m sh.Machine, hostName string, host Host, ws Workspace, here bool) (string, bool, error) {
	lines := []string{"What goes to " + hostName + ":"}
	secrets := nested(ws, func(w Workspace) []string { return w.Secrets })
	untracked := nested(ws, func(w Workspace) []string { return w.Untracked })
	size := total(ws, func(w Workspace) int64 { return w.Size })
	closing := "Nothing that's running goes."
	if ws.Git == nil {
		var err error
		if secrets, err = findSecrets(m, ws.Root, host.Secrets, host.Exclude, nil); err != nil {
			return "", false, err
		}
		out, err := folder(m, "size", ws.Root, pruneArgs(host.Exclude)...)
		if err != nil {
			return "", false, err
		}
		var files int
		if _, err := fmt.Sscan(string(out), &files, &size); err != nil {
			return "", false, fmt.Errorf("unexpected folder size %q from %s", out, m.Name())
		}
		lines = append(lines, fmt.Sprintf("  the folder, %d %s (%s), without %s", files, plural(files, "file"), humanSize(size), names(host.Exclude)))
	} else {
		branch := "branch " + ws.Git.Branch
		if ws.Git.Branch == "" {
			branch = "the detached HEAD"
		}
		n := total(ws, func(w Workspace) int { return w.Changed })
		lines = append(lines, fmt.Sprintf("  %s, its commits, and %d %s", branch, n, plural(n, "changed file")))
		if len(untracked) > 0 {
			lines = append(lines, "  files git doesn't track yet: "+names(untracked))
		}
		if size > 0 {
			lines = append(lines, "  changed and new files, up to "+humanSize(size))
		}
		if left := nested(ws, func(w Workspace) []string { return w.LeftOut }); len(left) > 0 {
			lines = append(lines, "  left out by exclude in magi's config: "+names(left))
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
		case project != nil && !approved:
			lines = append(lines, fmt.Sprintf("  from %s, nothing on this move: it isn't allowed here. It has %s. A Claude session here can let it run on later moves by calling allow_project with dir %s and sha256 %s.", projectFile, project.runs(), filepath.Dir(project.Path), project.Hash))
		case project != nil:
			lines = append(lines, "  from "+projectFile+": "+project.runs())
		}
	}
	lines = append(lines, "  the conversation and its project memory")
	ask := len(secrets) > 0 || len(untracked) > 0 || size >= askSize
	return strings.Join(append(lines, closing), "\n"), ask, nil
}

func describeSession(m sh.Machine, hostName string, host Host, sessionID string, here bool) (string, bool, error) {
	s, err := findSession(m, sessionID)
	if err != nil {
		return "", false, err
	}
	ws, err := sourceWorkspace(m, m.Name(), s.Cwd, host.Secrets, host.Exclude)
	if err != nil {
		return "", false, err
	}
	return describeCopy(m, hostName, host, ws, here)
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

func total[T int | int64](ws Workspace, of func(Workspace) T) T {
	n := of(ws)
	for _, sub := range ws.Submodules {
		n += total(sub, of)
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
