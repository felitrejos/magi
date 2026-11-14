package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

type Tab struct {
	Session        string
	Window         string
	Pane           string
	SessionCreated bool
}

type Launcher interface {
	Start(group, label, dir string, command []string) (Tab, error)
	Show(tab Tab) (shownOn []string, err error)
	Discard(tab Tab) error
	OpenProjectTab(tab Tab, name, dir, command string) error
	Where(group string) string
}

func newLauncher(h Host, remote *Remote) Launcher {
	if h.Launcher == "tmux" {
		return Tmux{remote: remote}
	}
	return Rex{server: h.RexServer, remote: remote}
}

func shellIn(dir string, command ...string) []string {
	if len(command) == 0 {
		return []string{"sh", "-c", "cd " + quote(dir) + ` && exec "$SHELL" -l`}
	}
	return []string{"sh", "-c", "cd " + quote(dir) + " && exec " + quoteAll(command)}
}

func claudeTabLabel(name string) string {
	return name + " · claude"
}

type Rex struct {
	server string
	remote *Remote
}

type rexSession struct {
	ID    string `json:"session_id"`
	Label string `json:"label"`
}

type rexWindow struct {
	ID       string   `json:"window_id"`
	Label    string   `json:"label"`
	BlockIDs []string `json:"block_ids"`
}

func (x Rex) rex(args ...string) ([]byte, error) {
	return run(x.remote.Context(), "rex", append([]string{"-S", x.server}, args...)...)
}

func (x Rex) rexJSON(into any, args ...string) error {
	out, err := x.rex(args...)
	if err != nil {
		return err
	}
	if err := json.Unmarshal(out, into); err != nil {
		return fmt.Errorf("rex %s: %w", strings.Join(args, " "), err)
	}
	return nil
}

func (x Rex) groupSession(group string) (string, error) {
	var listing struct {
		Sessions []rexSession `json:"sessions"`
	}
	if err := x.rexJSON(&listing, "ls", "--json"); err != nil {
		return "", err
	}
	for _, s := range listing.Sessions {
		if s.Label == group {
			return s.ID, nil
		}
	}
	return "", nil
}

func (x Rex) Start(group, label, dir string, command []string) (Tab, error) {
	session, err := x.groupSession(group)
	if err != nil {
		return Tab{}, err
	}
	claude := shellIn(dir, command...)
	if session == "" {
		var created struct {
			ID      string      `json:"session_id"`
			Windows []rexWindow `json:"initial_windows"`
		}
		if err := x.rexJSON(&created, append([]string{"session", "new", group, "--window", label, "--json", "--"}, claude...)...); err != nil {
			return Tab{}, err
		}
		tab := Tab{Session: created.ID, SessionCreated: true}
		if len(created.Windows) == 0 || len(created.Windows[0].BlockIDs) == 0 {
			return tab, errors.New("rex session new returned no tab")
		}
		tab.Window, tab.Pane = created.Windows[0].ID, created.Windows[0].BlockIDs[0]
		return tab, nil
	}
	var created rexWindow
	if err := x.rexJSON(&created, append([]string{"window", "new", "-s", session, label, "--json", "--"}, claude...)...); err != nil {
		return Tab{}, err
	}
	tab := Tab{Session: session, Window: created.ID}
	if len(created.BlockIDs) == 0 {
		return tab, errors.New("rex window new returned no pane")
	}
	tab.Pane = created.BlockIDs[0]
	return tab, nil
}

func (x Rex) OpenProjectTab(tab Tab, name, dir, command string) error {
	var windows struct {
		Windows []rexWindow `json:"windows"`
	}
	if err := x.rexJSON(&windows, "window", "ls", "-s", tab.Session, "--json"); err != nil {
		return err
	}
	for _, w := range windows.Windows {
		if w.Label == name {
			return nil
		}
	}
	var created rexWindow
	if err := x.rexJSON(&created, append([]string{"window", "new", "-s", tab.Session, name, "--focus=false", "--json", "--"}, shellIn(dir)...)...); err != nil {
		return err
	}
	if len(created.BlockIDs) == 0 {
		return errors.New("rex window new returned no pane")
	}
	if _, err := x.rex("send", "-s", tab.Session, "-b", created.BlockIDs[0], command); err != nil {
		return err
	}
	_, err := x.rex("send-key", "-s", tab.Session, "-b", created.BlockIDs[0], "enter")
	return err
}

func (x Rex) Show(tab Tab) ([]string, error) {
	var listing struct {
		Clients []struct {
			ID    string `json:"client_id"`
			State string `json:"connection_state"`
			Info  struct {
				Kind  string `json:"kind"`
				Label string `json:"label"`
			} `json:"info"`
		} `json:"clients"`
	}
	if err := x.rexJSON(&listing, "client", "ls", "--json"); err != nil {
		return nil, err
	}
	if _, err := x.rex("focus", "-s", tab.Session, tab.Pane); err != nil {
		return nil, err
	}
	var shownOn, refused []string
	for _, c := range listing.Clients {
		if c.Info.Kind != "app" || c.State != "connected" {
			continue
		}
		if _, err := x.rex("-C", c.ID, "do", "session.select", "session_id="+tab.Session, "window_id="+tab.Window); err != nil {
			refused = append(refused, fmt.Sprintf("%s (%v)", c.Info.Label, err))
			continue
		}
		shownOn = append(shownOn, "Rex on "+c.Info.Label)
	}
	if len(refused) > 0 {
		return shownOn, fmt.Errorf("Rex couldn't switch %s to it; turn on Remote Control in Rex's settings to allow that", strings.Join(refused, ", "))
	}
	return shownOn, nil
}

func (x Rex) Discard(tab Tab) error {
	var err error
	switch {
	case tab.SessionCreated:
		_, err = x.rex("kill", tab.Session)
	case tab.Window != "":
		_, err = x.rex("window", "close", "-s", tab.Session, tab.Window)
	}
	return err
}

func (x Rex) Where(group string) string {
	return fmt.Sprintf("in Rex on %s, session %s", x.remote.Name(), group)
}

type Tmux struct {
	remote *Remote
}

var tmuxName = strings.NewReplacer(".", "_", ":", "_").Replace

func (t Tmux) session(group string) (string, error) {
	out, err := sh(t.remote, `tmux list-sessions -F '#{session_id} #{session_name}' 2>/dev/null || :`)
	if err != nil {
		return "", err
	}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if id, name, ok := strings.Cut(line, " "); ok && name == tmuxName(group) {
			return id, nil
		}
	}
	return "", nil
}

func (t Tmux) Start(group, label, dir string, command []string) (Tab, error) {
	session, err := t.session(group)
	if err != nil {
		return Tab{}, err
	}
	argv := quoteAll(shellIn(dir, command...))
	script := "tmux new-session -d -P -F '#{session_id} #{window_id} #{pane_id}' -s " + quote(tmuxName(group)) + " -n " + quote(label) + " " + argv
	if session != "" {
		script = "tmux new-window -P -F '#{session_id} #{window_id} #{pane_id}' -t " + quote(session+":") + " -n " + quote(label) + " " + argv
	}
	out, err := sh(t.remote, script)
	if err != nil {
		return Tab{}, err
	}
	fields := strings.Fields(string(out))
	if len(fields) != 3 {
		return Tab{}, fmt.Errorf("unexpected tmux output %q", out)
	}
	return Tab{SessionCreated: session == "", Session: fields[0], Window: fields[1], Pane: fields[2]}, nil
}

func (t Tmux) OpenProjectTab(tab Tab, name, dir, command string) error {
	_, err := sh(t.remote, `set -e; if ! tmux list-windows -t `+quote(tab.Session)+` -F '#{window_name}' | grep -qxF -- `+quote(name)+`; then `+
		`p=$(tmux new-window -d -P -F '#{pane_id}' -t `+quote(tab.Session+":")+` -n `+quote(name)+` -c `+quote(dir)+`); `+
		`tmux send-keys -t "$p" -l `+quote(command)+`; tmux send-keys -t "$p" Enter; fi`)
	return err
}

func (t Tmux) Show(tab Tab) ([]string, error) {
	_, err := sh(t.remote, "tmux select-window -t "+quote(tab.Window))
	return nil, err
}

func (t Tmux) Discard(tab Tab) error {
	var err error
	switch {
	case tab.SessionCreated:
		_, err = sh(t.remote, "tmux kill-session -t "+quote(tab.Session))
	case tab.Window != "":
		_, err = sh(t.remote, "tmux kill-window -t "+quote(tab.Window))
	}
	return err
}

func (t Tmux) Where(group string) string {
	return fmt.Sprintf("in tmux on %s (ssh -t %s tmux attach -t %s)", t.remote.Name(), t.remote.Name(), quote("="+tmuxName(group)))
}
