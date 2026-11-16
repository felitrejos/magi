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
	chatFolder   = "Chats"
	chatGroup    = "chats"
	chatName     = "chat"
	openLimit    = 30 * time.Second
	listDepth    = 2
	listMaxFound = 200
)

type openReport struct {
	Event   string `json:"event"`
	OK      bool   `json:"ok"`
	Message string `json:"message"`
}

func openCommand(args []string, out io.Writer) error {
	asJSON, codex, list, chat := false, false, false, false
	resume, model := "", ""
	var rest []string
	for i := 0; i < len(args); i++ {
		switch a := args[i]; a {
		case "--json":
			asJSON = true
		case "--codex":
			codex = true
		case "--list":
			list = true
		case "--chat":
			chat = true
		case "--resume":
			if i+1 >= len(args) {
				return errors.New("--resume needs a session id")
			}
			i++
			resume = args[i]
		case "--model":
			if i+1 >= len(args) {
				return errors.New("--model needs a model name")
			}
			i++
			model = args[i]
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
	if list {
		if len(rest) != 0 || resume != "" || codex || model != "" || chat {
			return fail(errors.New("--list takes no folder"))
		}
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
	if chat {
		if len(rest) != 0 || resume != "" {
			return fail(errors.New("--chat starts in its own folder and continues nothing"))
		}
		home, err := os.UserHomeDir()
		if err != nil {
			return fail(err)
		}
		rest = []string{filepath.Join(home, chatFolder)}
		if err := os.MkdirAll(rest[0], 0o755); err != nil {
			return fail(err)
		}
	}
	if len(rest) != 1 {
		return fail(errors.New("usage: cielago open <folder> [--codex] [--model <model> | --resume <session-id>] [--json], cielago open --chat [--codex] [--model <model>] [--json], or cielago open --list [--json]"))
	}
	if resume != "" && (codex || !sessionIDPattern.MatchString(resume)) {
		return fail(fmt.Errorf("%q is not a Claude session id to continue", resume))
	}
	if model != "" && (resume != "" || !modelPattern.MatchString(model)) {
		return fail(fmt.Errorf("%q is not a model to start a new session with", model))
	}
	dir, err := filepath.Abs(rest[0])
	if err != nil {
		return fail(err)
	}
	if info, err := os.Stat(dir); err != nil || !info.IsDir() {
		return fail(fmt.Errorf("%s isn't a folder on this machine", dir))
	}
	root, group, name := dir, chatGroup, chatName
	if !chat {
		root = projectRoot(ctx, dir)
		if resume == "" && !inProjectFolders(root, roots) {
			return fail(fmt.Errorf("%s isn't in one of this machine's project folders", root))
		}
		if group, err = localGroup(root, roots); err != nil {
			return fail(err)
		}
		name = baseName(root)
	}
	if name == "" || name == "." || name == "/" || strings.HasPrefix(name, "-") {
		return fail(fmt.Errorf("%q can't name a session", name))
	}
	agent, label, command := "claude", claudeTabLabel(name), []string{"--remote-control=" + name}
	if resume != "" {
		command = []string{"--resume", resume, "--fork-session", "--remote-control=" + name}
	}
	if codex {
		agent, label, command = "codex", name+" · codex", nil
	}
	if model != "" {
		command = append(command, "--model", model)
	}
	program, err := exec.LookPath(agent)
	if err != nil {
		return fail(fmt.Errorf("%s isn't installed on %s", agent, local.Name()))
	}
	tab, err := Rex{remote: local}.Start(group, label, dir, append([]string{program}, command...), false)
	if err != nil {
		return fail(fmt.Errorf("Rex couldn't open the tab: %w", err))
	}
	what := "Opened"
	if resume != "" {
		what = fmt.Sprintf("Continued session %s as a copy in", shortID(resume))
	}
	where := fmt.Sprintf("%s %s in %s, Rex session %s", what, label, dir, group)
	if tab.SessionCreated {
		where += ", which is new"
	}
	return report(openReport{Event: "opened", OK: true, Message: where})
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

func inProjectFolders(path string, roots []string) bool {
	for _, root := range roots {
		if _, ok := groupUnderLocal(root, path); ok {
			return true
		}
	}
	return false
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
