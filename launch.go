package main

import (
	"encoding/json"
	"fmt"
)

type Launch struct {
	Host    Host
	Remote  *Remote
	Name    string
	Dir     string
	Command []string
}

func (l Launch) shellLine() string {
	return "cd " + quote(l.Dir) + " && exec " + quoteAll(l.Command)
}

func (l Launch) start() error {
	session, anchor, err := l.findRexWindow()
	if err != nil {
		return err
	}
	_, err = l.rex("split", "-s", session, "-b", anchor, "--focus=false",
		"--label", "claude: "+l.Name, "--json", "--", "sh", "-c", l.shellLine())
	return err
}

func (l Launch) rex(args ...string) ([]byte, error) {
	return run(l.Remote.ctx, "", nil, "rex", append([]string{"-S", l.Host.RexServer}, args...)...)
}

func (l Launch) findRexWindow() (session, anchor string, err error) {
	out, err := l.rex("ls", "--json")
	if err != nil {
		return "", "", err
	}
	var listing struct {
		Sessions []struct {
			Label string `json:"label"`
		} `json:"sessions"`
	}
	if err := json.Unmarshal(out, &listing); err != nil {
		return "", "", fmt.Errorf("rex ls: %w", err)
	}
	for _, s := range listing.Sessions {
		out, err := l.rex("window", "ls", "-s", s.Label, "--json")
		if err != nil {
			return "", "", err
		}
		var windows struct {
			Windows []struct {
				Label   string `json:"label"`
				Focused string `json:"focused_block_id"`
			} `json:"windows"`
		}
		if err := json.Unmarshal(out, &windows); err != nil {
			return "", "", fmt.Errorf("rex window ls -s %s: %w", s.Label, err)
		}
		for _, w := range windows.Windows {
			if w.Label == l.Name {
				return s.Label, w.Focused, nil
			}
		}
	}
	return "", "", fmt.Errorf("no Rex window named %q on %s", l.Name, l.Host.RexServer)
}
