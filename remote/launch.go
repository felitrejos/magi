package main

import (
	"errors"
	"fmt"
	"strings"
)

type Tab struct {
	Session        string
	Window         string
	Pane           string
	Tag            string
	SessionCreated bool
}

func shellIn(dir string, command ...string) []string {
	return append([]string{"sh", "-c", `cd -- "$1" && shift && PATH="$HOME/.local/bin:$PATH" exec "$@"`, "magi", dir}, command...)
}

func tabLabel(name string, codex bool) string {
	if codex {
		return name + " · codex"
	}
	return name + " · claude"
}

type Tmux struct {
	remote Machine
}

var tmuxName = strings.NewReplacer(".", "_", ":", "_").Replace

func (t Tmux) Start(group, label, dir string, command []string, focus bool) (Tab, error) {
	detached := "0"
	if !focus {
		detached = "1"
	}
	name, tag, shell := tmuxName(group), newSessionID(), shellIn(dir, command...)
	created := false
	out, err := sh(t.remote, tmuxScript, append([]string{"new-window", "=" + name, label, dir, tag, detached}, shell...)...)
	if err != nil && noTmuxSession(err) {
		created = true
		out, err = sh(t.remote, tmuxScript, append([]string{"new-session", name, label, dir, tag}, shell...)...)
	}
	if err != nil {
		return Tab{}, err
	}
	fields := strings.Fields(string(out))
	if len(fields) != 3 {
		if created {
			_, killErr := sh(t.remote, tmuxScript, "kill-session", "="+name)
			return Tab{}, errors.Join(fmt.Errorf("unexpected tmux output %q", out), killErr)
		}
		return Tab{}, fmt.Errorf("unexpected tmux output %q", out)
	}
	return Tab{SessionCreated: created, Session: fields[0], Window: fields[1], Pane: fields[2], Tag: tag}, nil
}

func noTmuxSession(err error) bool {
	msg := err.Error()
	return strings.Contains(msg, "can't find session") || strings.Contains(msg, "no server running") || strings.Contains(msg, "error connecting to")
}

func tabGone(err error) bool {
	return noTmuxSession(err) || strings.Contains(err.Error(), "can't find window")
}

func (t Tmux) Split(anchor string, below bool, dir string, command []string) (string, error) {
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
		_, err = sh(t.remote, tmuxScript, "kill-window", tab.Window, tab.Tag)
	}
	return err
}

func (t Tmux) Where(group string) string {
	return fmt.Sprintf("in tmux on %s (ssh -t %s tmux attach -t %s)", t.remote.Name(), t.remote.Name(), quote("="+tmuxName(group)))
}
