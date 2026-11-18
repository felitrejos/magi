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
	PanePID        string
	SessionCreated bool
}

type Launcher interface {
	Start(group, label, dir string, command []string, focus bool) (Tab, error)
	Show(tab Tab) (shownOn []string, err error)
	Discard(tab Tab) error
	Split(tab Tab, anchor string, below bool, label, dir string, command []string) (string, error)
	Where(group string) string
}

func newLauncher(h Host, remote *Remote) Launcher {
	if h.Launcher == "tmux" {
		return Tmux{remote: remote}
	}
	return Rex{server: h.RexServer, remote: remote}
}

func shellIn(dir string, command ...string) []string {
	return append([]string{"sh", "-c", `cd -- "$1" && shift && PATH="$HOME/.local/bin:$PATH" exec "$@"`, "magi", dir}, command...)
}

func claudeTabLabel(name string) string {
	return name + " · claude"
}

const (
	rexAPI     = 16
	rexRelease = "0.1.0-preview.20"
)

type Rex struct {
	server string
	remote Machine
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

func forwardVia(launcher Launcher, tab Tab) *rexVia {
	x, ok := launcher.(Rex)
	if !ok || x.server == "" || tab.Session == "" {
		return nil
	}
	return &rexVia{Server: x.server, Session: tab.Session}
}

func (x Rex) rex(args ...string) ([]byte, error) {
	if x.server == "" {
		return run(x.remote.Context(), "rex", args...)
	}
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

func (x Rex) Start(group, label, dir string, command []string, focus bool) (Tab, error) {
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
	args := []string{"window", "new", "-s", session, label, "--json"}
	if !focus {
		args = append(args, "--focus=false")
	}
	var created rexWindow
	if err := x.rexJSON(&created, append(append(args, "--"), claude...)...); err != nil {
		return Tab{}, err
	}
	tab := Tab{Session: session, Window: created.ID}
	if len(created.BlockIDs) == 0 {
		return tab, errors.New("rex window new returned no pane")
	}
	tab.Pane = created.BlockIDs[0]
	return tab, nil
}

func (x Rex) Split(tab Tab, anchor string, below bool, label, dir string, command []string) (string, error) {
	args := []string{"split", "-s", tab.Session, "-b", anchor, "--focus=false", "--label", label, "-c", dir, "--json"}
	if below {
		args = append(args, "--split", "below", "--ratio", "35")
	}
	var created struct {
		BlockIDs []string `json:"block_ids"`
	}
	if err := x.rexJSON(&created, append(append(args, "--"), command...)...); err != nil {
		return "", err
	}
	if len(created.BlockIDs) == 0 {
		return "", errors.New("rex split returned no pane")
	}
	return created.BlockIDs[0], nil
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
		if _, err = x.rex("window", "close", "-s", tab.Session, tab.Window); err == nil {
			err = x.dropIfEmpty(tab.Session)
		}
	}
	return err
}

func (x Rex) dropIfEmpty(session string) error {
	var listing struct {
		Windows []rexWindow `json:"windows"`
	}
	if err := x.rexJSON(&listing, "window", "ls", "-s", session, "--json"); err != nil {
		return err
	}
	if len(listing.Windows) > 0 {
		return nil
	}
	_, err := x.rex("kill", session)
	return err
}

func (x Rex) Where(group string) string {
	return fmt.Sprintf("in Rex on %s, session %s", x.remote.Name(), group)
}

type Tmux struct {
	remote Machine
}

var tmuxName = strings.NewReplacer(".", "_", ":", "_").Replace

func (t Tmux) session(group string) (string, error) {
	out, err := sh(t.remote, tmuxScript, "sessions")
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

func (t Tmux) Start(group, label, dir string, command []string, focus bool) (Tab, error) {
	session, err := t.session(group)
	if err != nil {
		return Tab{}, err
	}
	args := []string{"new-session", tmuxName(group), label, dir}
	if session != "" {
		detached := "0"
		if !focus {
			detached = "1"
		}
		args = []string{"new-window", session, label, dir, detached}
	}
	out, err := sh(t.remote, tmuxScript, append(args, shellIn(dir, command...)...)...)
	if err != nil {
		return Tab{}, err
	}
	fields := strings.Fields(string(out))
	if len(fields) != 4 {
		if session == "" {
			_, killErr := sh(t.remote, tmuxScript, "kill-session", "="+tmuxName(group))
			return Tab{}, errors.Join(fmt.Errorf("unexpected tmux output %q", out), killErr)
		}
		return Tab{}, fmt.Errorf("unexpected tmux output %q", out)
	}
	return Tab{SessionCreated: session == "", Session: fields[0], Window: fields[1], Pane: fields[2], PanePID: fields[3]}, nil
}

func (t Tmux) Split(tab Tab, anchor string, below bool, label, dir string, command []string) (string, error) {
	direction := "right"
	if below {
		direction = "below"
	}
	out, err := sh(t.remote, tmuxScript, append([]string{"split", anchor, direction, dir}, command...)...)
	if err != nil {
		return "", err
	}
	id := strings.TrimSpace(string(out))
	if !strings.HasPrefix(id, "%") {
		return "", fmt.Errorf("unexpected tmux output %q", out)
	}
	return id, nil
}

func (t Tmux) Show(tab Tab) ([]string, error) {
	out, err := sh(t.remote, tmuxScript, "select", tab.Window)
	if err != nil {
		return nil, err
	}
	if clients := strings.Fields(string(out)); len(clients) > 0 {
		return []string{fmt.Sprintf("tmux on %s (%d attached)", t.remote.Name(), len(clients))}, nil
	}
	return nil, nil
}

func (t Tmux) Discard(tab Tab) error {
	var err error
	switch {
	case tab.SessionCreated:
		_, err = sh(t.remote, tmuxScript, "kill-session", tab.Session)
	case tab.Window != "":
		_, err = sh(t.remote, tmuxScript, "kill-window", tab.Window, tab.PanePID)
	}
	return err
}

func (t Tmux) Where(group string) string {
	return fmt.Sprintf("in tmux on %s (ssh -t %s tmux attach -t %s)", t.remote.Name(), t.remote.Name(), quote("="+tmuxName(group)))
}
