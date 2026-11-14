package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

type Launch struct {
	Host    Host
	Remote  *Remote
	Group   string
	Name    string
	Dir     string
	Command []string
}

type RexTab struct {
	Session        string
	Window         string
	Shell          string
	Created        bool
	SessionCreated bool
	Pane           string
}

type rexSession struct {
	ID    string `json:"session_id"`
	Label string `json:"label"`
}

func (l Launch) rex(args ...string) ([]byte, error) {
	return run(l.Remote.ctx, "", nil, "rex", append([]string{"-S", l.Host.RexServer}, args...)...)
}

func (l Launch) rexJSON(into any, args ...string) error {
	out, err := l.rex(args...)
	if err != nil {
		return err
	}
	if err := json.Unmarshal(out, into); err != nil {
		return fmt.Errorf("rex %s: %w", strings.Join(args, " "), err)
	}
	return nil
}

func shellIn(dir string, command ...string) []string {
	if len(command) == 0 {
		return []string{"sh", "-c", "cd " + quote(dir) + ` && exec "$SHELL" -l`}
	}
	return []string{"sh", "-c", "cd " + quote(dir) + " && exec " + quoteAll(command)}
}

func (l Launch) start(repoDir string) (RexTab, error) {
	tab, err := l.findTab()
	if err != nil {
		return tab, err
	}
	if tab.Window == "" {
		if tab, err = l.createTab(tab.Session, repoDir); err != nil {
			return tab, err
		}
	}
	var split struct {
		BlockIDs []string `json:"block_ids"`
	}
	err = l.rexJSON(&split, append([]string{"split", "-s", tab.Session, "-b", tab.Shell,
		"--label", "claude: " + l.Name, "--json", "--"}, shellIn(l.Dir, l.Command...)...)...)
	if err != nil {
		return tab, err
	}
	if len(split.BlockIDs) == 0 {
		return tab, errors.New("rex split returned no pane")
	}
	tab.Pane = split.BlockIDs[0]
	return tab, nil
}

func (l Launch) findTab() (RexTab, error) {
	var listing struct {
		Sessions []rexSession `json:"sessions"`
	}
	if err := l.rexJSON(&listing, "ls", "--json"); err != nil {
		return RexTab{}, err
	}
	for _, s := range listing.Sessions {
		if s.Label != l.Group {
			continue
		}
		var windows struct {
			Windows []struct {
				ID      string `json:"window_id"`
				Label   string `json:"label"`
				Focused string `json:"focused_block_id"`
			} `json:"windows"`
		}
		if err := l.rexJSON(&windows, "window", "ls", "-s", s.ID, "--json"); err != nil {
			return RexTab{}, err
		}
		for _, w := range windows.Windows {
			if w.Label == l.Name {
				return RexTab{Session: s.ID, Window: w.ID, Shell: w.Focused}, nil
			}
		}
		return RexTab{Session: s.ID}, nil
	}
	return RexTab{}, nil
}

func (l Launch) createTab(session, repoDir string) (RexTab, error) {
	shell := shellIn(repoDir)
	if session == "" {
		var created struct {
			ID      string `json:"session_id"`
			Windows []struct {
				ID       string   `json:"window_id"`
				BlockIDs []string `json:"block_ids"`
			} `json:"initial_windows"`
		}
		args := append([]string{"session", "new", l.Group, "--window", l.Name, "--json", "--"}, shell...)
		if err := l.rexJSON(&created, args...); err != nil {
			return RexTab{}, err
		}
		tab := RexTab{Session: created.ID, Created: true, SessionCreated: true}
		if len(created.Windows) == 0 || len(created.Windows[0].BlockIDs) == 0 {
			return tab, errors.New("rex session new returned no tab")
		}
		tab.Window, tab.Shell = created.Windows[0].ID, created.Windows[0].BlockIDs[0]
		return tab, nil
	}
	var created struct {
		ID       string   `json:"window_id"`
		BlockIDs []string `json:"block_ids"`
	}
	args := append([]string{"window", "new", "-s", session, l.Name, "--json", "--"}, shell...)
	if err := l.rexJSON(&created, args...); err != nil {
		return RexTab{}, err
	}
	tab := RexTab{Session: session, Window: created.ID, Created: true}
	if len(created.BlockIDs) == 0 {
		return tab, errors.New("rex window new returned no pane")
	}
	tab.Shell = created.BlockIDs[0]
	return tab, nil
}

func (l Launch) show(tab RexTab) ([]string, error) {
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
	if err := l.rexJSON(&listing, "client", "ls", "--json"); err != nil {
		return nil, err
	}
	if _, err := l.rex("focus", "-s", tab.Session, tab.Pane); err != nil {
		return nil, err
	}
	var shownOn, refused []string
	for _, c := range listing.Clients {
		if c.Info.Kind != "app" || c.State != "connected" {
			continue
		}
		if _, err := l.rex("-C", c.ID, "do", "session.select", "session_id="+tab.Session, "window_id="+tab.Window); err != nil {
			refused = append(refused, fmt.Sprintf("%s (%v)", c.Info.Label, err))
			continue
		}
		shownOn = append(shownOn, c.Info.Label)
	}
	if len(refused) > 0 {
		return shownOn, fmt.Errorf("Rex couldn't switch %s to it; turn on Remote Control in Rex's settings to allow that", strings.Join(refused, ", "))
	}
	return shownOn, nil
}

func (l Launch) discard(tab RexTab) error {
	var err error
	switch {
	case tab.SessionCreated:
		_, err = l.rex("kill", tab.Session)
	case tab.Created:
		_, err = l.rex("window", "close", "-s", tab.Session, tab.Window)
	case tab.Pane != "":
		_, err = l.rex("block", "close", "-s", tab.Session, tab.Pane)
	}
	return err
}

func (l Launch) runInShell(tab RexTab, command string) error {
	if _, err := l.rex("send", "-s", tab.Session, "-b", tab.Shell, command); err != nil {
		return err
	}
	_, err := l.rex("send-key", "-s", tab.Session, "-b", tab.Shell, "enter")
	return err
}
