package main

import (
	"encoding/json"
	"fmt"
	"slices"
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

func (l Launch) shellIn(dir string, command ...string) []string {
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
	if tab.Session == "" {
		if tab, err = l.createTab(repoDir); err != nil {
			return tab, err
		}
	}
	var split struct {
		BlockIDs []string `json:"block_ids"`
	}
	err = l.rexJSON(&split, append([]string{"split", "-s", tab.Session, "-b", tab.Shell,
		"--label", "claude: " + l.Name, "--json", "--"}, l.shellIn(l.Dir, l.Command...)...)...)
	if err != nil {
		return tab, err
	}
	tab.Pane = split.BlockIDs[0]
	return tab, nil
}

func (l Launch) findTab() (RexTab, error) {
	var listing struct {
		Sessions []struct {
			ID    string `json:"session_id"`
			Label string `json:"label"`
		} `json:"sessions"`
	}
	if err := l.rexJSON(&listing, "ls", "--json"); err != nil {
		return RexTab{}, err
	}
	slices.SortStableFunc(listing.Sessions, func(a, b struct {
		ID    string `json:"session_id"`
		Label string `json:"label"`
	}) int {
		return boolOrder(b.Label == l.Group) - boolOrder(a.Label == l.Group)
	})
	for _, s := range listing.Sessions {
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
	}
	return RexTab{}, nil
}

func (l Launch) createTab(repoDir string) (RexTab, error) {
	session, err := l.groupSession()
	if err != nil {
		return RexTab{}, err
	}
	shell := l.shellIn(repoDir)
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
		return RexTab{Session: created.ID, Window: created.Windows[0].ID, Shell: created.Windows[0].BlockIDs[0], Created: true, SessionCreated: true}, nil
	}
	var created struct {
		ID       string   `json:"window_id"`
		BlockIDs []string `json:"block_ids"`
	}
	args := append([]string{"window", "new", "-s", session, l.Name, "--json", "--"}, shell...)
	if err := l.rexJSON(&created, args...); err != nil {
		return RexTab{}, err
	}
	return RexTab{Session: session, Window: created.ID, Shell: created.BlockIDs[0], Created: true}, nil
}

func (l Launch) groupSession() (string, error) {
	var listing struct {
		Sessions []struct {
			ID    string `json:"session_id"`
			Label string `json:"label"`
		} `json:"sessions"`
	}
	if err := l.rexJSON(&listing, "ls", "--json"); err != nil {
		return "", err
	}
	for _, s := range listing.Sessions {
		if s.Label == l.Group {
			return s.ID, nil
		}
	}
	return "", nil
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
	var shownOn []string
	var refused []string
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
	switch {
	case tab.SessionCreated:
		_, err := l.rex("kill", tab.Session)
		return err
	case tab.Created:
		_, err := l.rex("window", "close", "-s", tab.Session, tab.Window)
		return err
	}
	return nil
}

func (l Launch) runInShell(tab RexTab, command string) error {
	if _, err := l.rex("send", "-s", tab.Session, "-b", tab.Shell, command); err != nil {
		return err
	}
	_, err := l.rex("send-key", "-s", tab.Session, "-b", tab.Shell, "enter")
	return err
}

func boolOrder(b bool) int {
	if b {
		return 1
	}
	return 0
}
