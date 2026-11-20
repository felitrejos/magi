package main

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/BurntSushi/toml"
	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const projectFile = ".magi.toml"

const projectPanesLimit = 30 * time.Second

type Project struct {
	Path     string   `toml:"-"`
	Hash     string   `toml:"-"`
	Setup    string   `toml:"setup"`
	Services []string `toml:"services"`
	Ports    []int    `toml:"ports"`
}

func parseProject(data []byte) (*Project, error) {
	var p Project
	md, err := toml.Decode(string(data), &p)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", projectFile, err)
	}
	if extra := md.Undecoded(); len(extra) > 0 {
		return nil, fmt.Errorf("%s: unknown key %q; it takes setup, services and ports", projectFile, extra[0].String())
	}
	if md.IsDefined("setup") && strings.TrimSpace(p.Setup) == "" {
		return nil, fmt.Errorf("%s: setup can't be empty", projectFile)
	}
	if p.Setup == "" && len(p.Services) == 0 && len(p.Ports) == 0 {
		return nil, fmt.Errorf("%s has no setup, services or ports", projectFile)
	}
	seen := map[int]bool{}
	for _, port := range p.Ports {
		if port < 1 || port > 65535 {
			return nil, fmt.Errorf("%s: %d isn't a port", projectFile, port)
		}
		if seen[port] {
			return nil, fmt.Errorf("%s: port %d is listed twice", projectFile, port)
		}
		seen[port] = true
	}
	for _, command := range append([]string{p.Setup}, p.Services...) {
		if strings.ContainsFunc(command, func(r rune) bool { return unicode.IsControl(r) || unicode.Is(unicode.Cf, r) }) {
			return nil, fmt.Errorf("%s: commands can't contain control or format characters", projectFile)
		}
	}
	for _, s := range p.Services {
		if strings.TrimSpace(s) == "" {
			return nil, fmt.Errorf("%s: services can't be empty", projectFile)
		}
	}
	sum := sha256.Sum256(data)
	p.Hash = hex.EncodeToString(sum[:])
	return &p, nil
}

func readProject(root string) (*Project, error) {
	real, err := filepath.EvalSymlinks(root)
	if err != nil {
		return nil, err
	}
	path := filepath.Join(real, projectFile)
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	p, err := parseProject(data)
	if err != nil {
		return nil, err
	}
	p.Path = path
	return p, nil
}

func approvedProject(local *sh.Local, root string) (*Project, error) {
	p, approved, err := readApproved(root)
	if p == nil || err != nil {
		return nil, err
	}
	if !approved {
		return nil, fmt.Errorf("%s isn't allowed on %s yet, so it didn't run. %s", p.Path, local.Name(), p.toAllow())
	}
	return p, nil
}

func (p *Project) runs() string {
	var runs []string
	if p.Setup != "" {
		runs = append(runs, fmt.Sprintf("setup %q", p.Setup))
	}
	for _, s := range p.Services {
		runs = append(runs, fmt.Sprintf("service %q", s))
	}
	if len(p.Ports) > 0 {
		ports := make([]string, len(p.Ports))
		for i, port := range p.Ports {
			ports[i] = strconv.Itoa(port)
		}
		runs = append(runs, plural(len(ports), "port")+" "+strings.Join(ports, ", "))
	}
	return strings.Join(runs, ", ")
}

func (p *Project) toAllow() string {
	return fmt.Sprintf("It has %s. To let it run, call allow_project with dir %s and sha256 %s: magi shows the user these commands and allows them only if the user accepts.", p.runs(), filepath.Dir(p.Path), p.Hash)
}

func readApproved(root string) (*Project, bool, error) {
	p, err := readProject(root)
	if p == nil || err != nil {
		return nil, false, err
	}
	approvals, err := readApprovals()
	if err != nil {
		return nil, false, err
	}
	return p, approvals[p.Path] == p.Hash, nil
}

func readApprovals() (map[string]string, error) {
	path, err := store.Dir("allowed")
	if err != nil {
		return nil, err
	}
	approvals := map[string]string{}
	f, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return approvals, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		if hash, file, ok := strings.Cut(scanner.Text(), "\t"); ok {
			approvals[file] = hash
		}
	}
	return approvals, scanner.Err()
}

func writeApprovals(approvals map[string]string) error {
	path, err := store.Dir("allowed")
	if err != nil {
		return err
	}
	var b strings.Builder
	for file, hash := range approvals {
		fmt.Fprintf(&b, "%s\t%s\n", hash, file)
	}
	return store.WriteAtomic(path, []byte(b.String()))
}

func allowProject(dir, hash string, answers mcp.InputResponseMap) (*mcp.CallToolResult, error) {
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return nil, err
	}
	ws, err := readWorkspace(local, dir, nil, nil)
	if err != nil {
		return nil, err
	}
	p, err := readProject(ws.Root)
	if err != nil {
		return nil, err
	}
	if p == nil {
		return nil, fmt.Errorf("no %s at %s, the project's root", projectFile, ws.Root)
	}
	if p.Hash != hash {
		return nil, fmt.Errorf("%s doesn't have sha256 %s, so it isn't what the user saw; nothing was allowed. %s", p.Path, hash, p.toAllow())
	}
	answer, asked := answers[p.Hash].(*mcp.ElicitResult)
	if !asked {
		return &mcp.CallToolResult{InputRequests: mcp.InputRequestMap{p.Hash: &mcp.ElicitParams{
			Message:         fmt.Sprintf("Allow %s? It has %s. magi runs these on the machine a session of this project moves to from %s, here when %s brings one back, and before run_elsewhere and delegate tasks started from here. If the file changes, magi asks again.", p.Path, p.runs(), local.Name(), local.Name()),
			RequestedSchema: map[string]any{"type": "object", "properties": map[string]any{}},
		}}}, nil
	}
	if answer.Action != "accept" {
		return said(fmt.Sprintf("The user didn't allow it (%s), so nothing was allowed.", answer.Action)), nil
	}
	approvals, err := readApprovals()
	if err != nil {
		return nil, err
	}
	approvals[p.Path] = p.Hash
	if err := writeApprovals(approvals); err != nil {
		return nil, err
	}
	return said(fmt.Sprintf("Allowed %s for moves and tasks started from %s, wherever they run. If it changes, it needs allowing again.", p.Path, local.Name())), nil
}

func prepareSetupStatus(m sh.Machine, sessionID string) (string, error) {
	out, err := sh.Run(m, `mkdir -p "$HOME/.magi/setup" && f="$HOME/.magi/setup/$1" && : > "$f" && printf '%s' "$f"`, sessionID)
	return string(out), err
}

func projectPrompt(p *Project, status string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "magi is opening panes beside this one for the project's %s: ", projectFile)
	if p.Setup != "" {
		fmt.Fprintf(&b, "setup (%s), which writes ok or failed to %s when it finishes", p.Setup, status)
		if len(p.Services) > 0 {
			b.WriteString(", then ")
		}
	}
	if len(p.Services) > 0 {
		fmt.Fprintf(&b, "each service in its own pane (%s)", strings.Join(p.Services, "; "))
		if p.Setup != "" {
			b.WriteString(" once setup succeeds")
		}
	}
	b.WriteString(". Leave those to the panes rather than starting them yourself, unless they failed. ")
	return b.String()
}

func startProject(l sh.Tmux, tab sh.Tab, dir, status string, p *Project) error {
	ctx, cancel := context.WithTimeout(context.Background(), projectPanesLimit)
	defer cancel()
	l.Machine = l.Machine.With(ctx)
	type pane struct{ role, command string }
	var panes []pane
	if p.Setup != "" {
		panes = append(panes, pane{"setup", p.Setup})
	}
	for _, s := range p.Services {
		panes = append(panes, pane{"service", s})
	}
	marker := ""
	if p.Setup != "" {
		marker = status
	}
	anchor, below := tab.Pane, true
	for _, pn := range panes {
		id, err := l.Split(anchor, below, dir, sh.InDir(dir, "sh", "-c", projectScript, "magi", pn.role, marker, pn.command))
		if err != nil {
			return fmt.Errorf("the project's panes didn't open: %w", err)
		}
		anchor, below = id, false
	}
	return nil
}
