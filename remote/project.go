package main

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/BurntSushi/toml"
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
		if strings.ContainsFunc(command, func(r rune) bool { return r < 0x20 || r == 0x7f }) {
			return nil, fmt.Errorf("%s: commands can't contain control characters", projectFile)
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

func approvedProject(local *Local, root string) (*Project, error) {
	p, err := readProject(root)
	if p == nil || err != nil {
		return nil, err
	}
	approvals, err := readApprovals()
	if err != nil {
		return nil, err
	}
	if approvals[p.Path] != p.Hash {
		return nil, fmt.Errorf("%s isn't approved on %s, so it didn't run; check it and run magi allow in %s", p.Path, local.Name(), filepath.Dir(p.Path))
	}
	return p, nil
}

func readApprovals() (map[string]string, error) {
	path, err := magiDir("allowed")
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
	path, err := magiDir("allowed")
	if err != nil {
		return err
	}
	var b strings.Builder
	for file, hash := range approvals {
		fmt.Fprintf(&b, "%s\t%s\n", hash, file)
	}
	return writeAtomic(path, []byte(b.String()))
}

func allowProject(dir string, in io.Reader, out io.Writer) error {
	local, err := newLocal(context.Background())
	if err != nil {
		return err
	}
	ws, err := readWorkspace(local, dir, nil, nil)
	if err != nil {
		return err
	}
	p, err := readProject(ws.Root)
	if err != nil {
		return err
	}
	if p == nil {
		return fmt.Errorf("no %s at %s, the project's root", projectFile, ws.Root)
	}
	fmt.Fprintf(out, "%s runs this on the machine a session moves to:\n", p.Path)
	if p.Setup != "" {
		fmt.Fprintf(out, "  setup:   %s\n", p.Setup)
	}
	for _, s := range p.Services {
		fmt.Fprintf(out, "  service: %s\n", s)
	}
	for _, port := range p.Ports {
		fmt.Fprintf(out, "  port:    %d, which the machine it leaves forwards to it\n", port)
	}
	fmt.Fprint(out, "Allow it? [y/N] ")
	answer, err := bufio.NewReader(in).ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		return err
	}
	if a := strings.ToLower(strings.TrimSpace(answer)); a != "y" && a != "yes" {
		fmt.Fprintln(out, "Not allowed.")
		return nil
	}
	approvals, err := readApprovals()
	if err != nil {
		return err
	}
	approvals[p.Path] = p.Hash
	if err := writeApprovals(approvals); err != nil {
		return err
	}
	fmt.Fprintln(out, "Allowed. Change the file and it needs allowing again.")
	return nil
}

func prepareSetupStatus(m Machine, sessionID string) (string, error) {
	out, err := sh(m, `mkdir -p "$HOME/.magi/setup" && f="$HOME/.magi/setup/$1" && : > "$f" && printf '%s' "$f"`, sessionID)
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

func startProject(m Machine, l Tmux, tab Tab, dir, status string, p *Project) error {
	parent := m.Context()
	ctx, cancel := context.WithTimeout(context.Background(), projectPanesLimit)
	defer cancel()
	m.SetContext(ctx)
	defer m.SetContext(parent)
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
		id, err := l.Split(anchor, below, dir, shellIn(dir, "sh", "-c", projectScript, "magi", pn.role, marker, pn.command))
		if err != nil {
			return fmt.Errorf("the project's panes didn't open: %w", err)
		}
		anchor, below = id, false
	}
	return nil
}

func recordTab(m Machine, sessionID string, tab Tab) error {
	_, err := sh(m, `mkdir -p "$HOME/.magi/tabs" && printf '%s\n%s\n%s\n' "$2" "$3" "$4" > "$HOME/.magi/tabs/$1"`, sessionID, tab.Session, tab.Window, tab.Tag)
	return err
}

func closeRecordedTab(r *Remote, sessionID string) error {
	out, err := sh(r, `f="$HOME/.magi/tabs/$1"; if [ -f "$f" ]; then cat "$f"; rm -f "$f"; fi`, sessionID)
	if err != nil || len(out) == 0 {
		return err
	}
	lines := strings.Split(string(out), "\n")
	if len(lines) < 3 {
		return fmt.Errorf("unexpected tab record for session %s on %s", sessionID, r.Name())
	}
	if err := (Tmux{remote: r}).Discard(Tab{Session: lines[0], Window: lines[1], Tag: lines[2]}); err != nil && !tabGone(err) {
		return err
	}
	return nil
}
