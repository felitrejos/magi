package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

const (
	openLimit    = 30 * time.Second
	listDepth    = 2
	listMaxFound = 200
)

type openReport struct {
	Event   string `json:"event"`
	OK      bool   `json:"ok"`
	Dir     string `json:"dir,omitempty"`
	Message string `json:"message"`
}

func openCommand(args []string, out io.Writer) error {
	asJSON, codex, list := false, false, false
	var rest []string
	for _, a := range args {
		switch a {
		case "--json":
			asJSON = true
		case "--codex":
			codex = true
		case "--list":
			list = true
		default:
			rest = append(rest, a)
		}
	}
	report := func(r openReport) error {
		if asJSON {
			return json.NewEncoder(out).Encode(r)
		}
		_, err := fmt.Fprintln(out, r.Message)
		return err
	}
	fail := func(err error) error {
		if reportErr := report(openReport{Event: "failed", Message: err.Error()}); reportErr != nil {
			return reportErr
		}
		return errReported
	}
	ctx, cancel := context.WithTimeout(context.Background(), openLimit)
	defer cancel()
	local, err := newLocal(ctx)
	if err != nil {
		return fail(err)
	}
	roots, err := openRoots(local)
	if err != nil {
		return fail(err)
	}
	if list && len(rest) == 0 {
		folders, err := knownProjects(roots)
		if err != nil {
			return fail(err)
		}
		if asJSON {
			return json.NewEncoder(out).Encode(folders)
		}
		_, err = fmt.Fprintln(out, strings.Join(folders, "\n"))
		return err
	}
	if len(rest) != 1 {
		return fail(errors.New("usage: cielago open <folder> [--codex] [--json], or cielago open --list [--json]"))
	}
	dir, err := filepath.Abs(rest[0])
	if err != nil {
		return fail(err)
	}
	if info, err := os.Stat(dir); err != nil || !info.IsDir() {
		return fail(fmt.Errorf("%s isn't a folder on this machine", dir))
	}
	root := projectRoot(ctx, dir)
	group, err := localGroup(root, roots)
	if err != nil {
		return fail(err)
	}
	name := baseName(root)
	agent, label, command := "claude", claudeTabLabel(name), []string{"--remote-control", name}
	if codex {
		agent, label, command = "codex", name+" · codex", nil
	}
	program, err := exec.LookPath(agent)
	if err != nil {
		return fail(fmt.Errorf("%s isn't installed on %s", agent, local.Name()))
	}
	tab, err := Rex{remote: local}.Start(group, label, dir, append([]string{program}, command...), false)
	if err != nil {
		return fail(fmt.Errorf("Rex couldn't open the tab: %w", err))
	}
	where := fmt.Sprintf("Opened %s in %s, Rex session %s", label, dir, group)
	if tab.SessionCreated {
		where += ", which is new"
	}
	return report(openReport{Event: "opened", OK: true, Dir: dir, Message: where})
}

func openRoots(m Machine) ([]string, error) {
	cfg, _, err := loadConfig()
	if errors.Is(err, os.ErrNotExist) {
		return existing(m, projectFolders), nil
	}
	if err != nil {
		return nil, err
	}
	return cfg.LocalRoots, nil
}

func projectRoot(ctx context.Context, dir string) string {
	out, err := run(ctx, "git", "-C", dir, "rev-parse", "--show-toplevel")
	if root := strings.TrimSpace(string(out)); err == nil && root != "" {
		return root
	}
	return dir
}

func knownProjects(localRoots []string) ([]string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, err
	}
	var found []string
	var walk func(dir string, depth int)
	walk = func(dir string, depth int) {
		if len(found) >= listMaxFound {
			return
		}
		if _, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
			found = append(found, dir)
			return
		}
		if depth == listDepth {
			return
		}
		entries, err := os.ReadDir(dir)
		if err != nil {
			return
		}
		for _, entry := range entries {
			if entry.IsDir() && !strings.HasPrefix(entry.Name(), ".") {
				walk(filepath.Join(dir, entry.Name()), depth+1)
			}
		}
	}
	for _, root := range localRoots {
		walk(expandHome(root, home), 0)
	}
	sort.Strings(found)
	return found, nil
}
