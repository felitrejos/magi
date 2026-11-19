package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"time"
)

var testedClaude = [2]string{"2.1.319", "2.1.326"}

var minGit = [3]int{2, 31, 0}

type checks struct {
	out    io.Writer
	failed int
}

func (c *checks) pass(what string) {
	fmt.Fprintf(c.out, "  ✓ %s\n", what)
}

func (c *checks) fail(what, fix string) {
	c.failed++
	fmt.Fprintf(c.out, "  ✗ %s\n      %s\n", what, fix)
}

func (c *checks) warn(what, note string) {
	fmt.Fprintf(c.out, "  ! %s\n      %s\n", what, note)
}

func (c *checks) check(err error, what, fix string) bool {
	if err != nil {
		c.fail(what, fmt.Sprintf("%s (%v)", fix, err))
		return false
	}
	c.pass(what)
	return true
}

func parseVersion(text string) ([3]int, bool) {
	var v [3]int
	words := strings.Fields(text)
	if len(words) == 0 {
		return v, false
	}
	parts := strings.SplitN(words[0], ".", 3)
	if len(parts) != 3 {
		return v, false
	}
	for i, p := range parts {
		n, err := strconv.Atoi(p)
		if err != nil {
			return v, false
		}
		v[i] = n
	}
	return v, true
}

func compareVersions(a, b [3]int) int {
	for i := range a {
		if a[i] != b[i] {
			return a[i] - b[i]
		}
	}
	return 0
}

func (c *checks) claudeVersion(where, text string) {
	v, ok := parseVersion(strings.TrimSpace(text))
	low, _ := parseVersion(testedClaude[0])
	high, _ := parseVersion(testedClaude[1])
	switch {
	case !ok:
		c.warn("Claude Code version on "+where+" is unreadable: "+strings.TrimSpace(text), "magi was tested with "+testedClaude[0]+" to "+testedClaude[1]+".")
	case compareVersions(v, low) < 0:
		c.fail("Claude Code on "+where+" is "+strings.TrimSpace(text), "update it with claude update; magi needs "+testedClaude[0]+" or newer")
	case compareVersions(v, high) > 0:
		c.warn("Claude Code on "+where+" is "+strings.TrimSpace(text)+", newer than magi was tested with ("+testedClaude[1]+")",
			"magi reads some Claude Code internals; if a handoff misbehaves, look for a magi update.")
	default:
		c.pass("Claude Code " + strings.TrimSpace(strings.TrimSuffix(strings.TrimSpace(text), "(Claude Code)")) + " on " + where)
	}
}

func (c *checks) gitVersion(where, text string, err error) {
	if err != nil {
		c.fail("git isn't installed on "+where, "install git 2.31 or newer")
		return
	}
	words := append(strings.Fields(text), "", "", "")
	v, ok := parseVersion(words[2])
	switch {
	case !ok:
		c.warn("git's version on "+where+" is unreadable: "+strings.TrimSpace(text), "magi needs git 2.31 or newer.")
	case compareVersions(v, minGit) < 0:
		c.fail("git on "+where+" is "+words[2], "magi needs git 2.31 or newer; update it")
	default:
		c.pass("git " + words[2] + " on " + where)
	}
}

func (c *checks) login(where, statusJSON string) {
	var status struct {
		LoggedIn   bool   `json:"loggedIn"`
		AuthMethod string `json:"authMethod"`
	}
	if err := json.Unmarshal([]byte(statusJSON), &status); err != nil || !status.LoggedIn {
		c.fail("Claude Code isn't logged in on "+where, "run claude auth login there")
		return
	}
	if status.AuthMethod != "claude.ai" {
		c.fail("Claude Code on "+where+" uses "+status.AuthMethod+" authentication", "Remote Control needs a claude.ai login: run claude auth login there")
		return
	}
	c.pass("logged in to claude.ai on " + where)
}

func (c *checks) plugin(listJSON string) {
	var plugins []struct {
		ID      string `json:"id"`
		Enabled bool   `json:"enabled"`
	}
	if err := json.Unmarshal([]byte(listJSON), &plugins); err != nil {
		c.warn("couldn't read claude plugin list --json: "+err.Error(), "check that the magi plugin is installed and enabled with claude plugin list")
		return
	}
	disabled := ""
	for _, p := range plugins {
		if !strings.HasPrefix(p.ID, "magi@") {
			continue
		}
		if p.Enabled {
			c.pass("the magi plugin is installed and enabled")
			return
		}
		disabled = p.ID
	}
	if disabled != "" {
		c.fail("the magi plugin is installed but disabled", "claude plugin enable "+disabled)
		return
	}
	c.fail("the magi plugin isn't installed", "claude plugin marketplace add felitrejos/magi && claude plugin install magi@magi")
}

func doctor(names []string, out io.Writer) error {
	c := &checks{out: out}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	local, err := newLocal(ctx)
	if err != nil {
		return err
	}
	fmt.Fprintf(out, "This machine (%s)\n", local.Name())
	cfg, path, err := loadConfig()
	if !c.check(err, "config at "+path, "run magi init to create it") {
		return errors.New("fix the config first")
	}
	gitOut, gitErr := run(ctx, "git", "--version")
	c.gitVersion("this machine", string(gitOut), gitErr)
	for _, tool := range []string{"tar", "ssh"} {
		_, err := exec.LookPath(tool)
		c.check(err, tool+" is installed", "install "+tool)
	}
	if _, err := exec.LookPath("magi"); err != nil {
		c.fail("magi isn't on PATH", "the Claude Code hook runs magi by name; put its folder on PATH")
	} else {
		c.pass("magi is on PATH")
	}
	if v, err := run(ctx, "claude", "--version"); c.check(err, "claude is installed", "install Claude Code") {
		c.claudeVersion("this machine", string(v))
		status, _ := run(ctx, "claude", "auth", "status")
		c.login("this machine", string(status))
		plugins, _ := run(ctx, "claude", "plugin", "list", "--json")
		c.plugin(string(plugins))
	}
	if watchRunning() {
		c.pass("magi watch runs here, so sessions on other machines can be handed back")
	} else {
		c.warn("magi watch isn't running here", "run magi watch install so a session on another machine can be handed back here")
	}
	if fleet, err := readFleet(ctx); err != nil {
		c.warn("the overview doesn't report machine load ("+err.Error()+")", "tasks sent to other machines go to the first configured host that answers; when the overview is installed, their load is also checked against each host's budget.")
	} else {
		c.pass(fmt.Sprintf("magi reports load for %d machine(s)", len(fleet.Machines)))
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	for _, root := range cfg.LocalRoots {
		_, err := os.Stat(expandHome(root, home))
		c.check(err, "local root "+root+" exists", "fix local_roots in "+path)
	}
	if len(names) == 0 {
		names = cfg.Order
	}
	for _, name := range names {
		fmt.Fprintf(out, "\n%s\n", name)
		h, err := loadHost(name)
		if !c.check(err, "config for "+name, "edit "+path) {
			continue
		}
		b, _ := h.budget()
		fmt.Fprintf(out, "  · budget for tasks: %s\n", b)
		if reason := b.noRoom(time.Now(), nil, true); reason != "" {
			fmt.Fprintf(out, "  · no new tasks now: %s\n", reason)
		}
		hostCtx, cancelHost := context.WithTimeout(context.Background(), 60*time.Second)
		doctorHost(hostCtx, c, name, h)
		cancelHost()
	}
	if c.failed > 0 {
		return fmt.Errorf("%d check(s) failed", c.failed)
	}
	fmt.Fprintln(out, "\nAll good.")
	return nil
}

func doctorHost(ctx context.Context, c *checks, name string, h Host) {
	remote, err := newRemote(ctx, h)
	if err != nil {
		c.fail("ssh setup", err.Error())
		return
	}
	if _, err := sh(remote, "true"); !c.check(err, "ssh "+h.SSH+" works without a password prompt", "set up key-based ssh to "+h.SSH) {
		return
	}
	gitOut, gitErr := sh(remote, "git --version")
	c.gitVersion(name, string(gitOut), gitErr)
	_, err = sh(remote, "command -v tar >/dev/null")
	c.check(err, "tar on "+name, "install tar there")
	home, err := homeDir(remote)
	if err != nil {
		c.fail("read $HOME on "+name, err.Error())
		return
	}
	claude := expandHome(h.Claude, home)
	if v, err := sh(remote, `"$1" --version`, claude); c.check(err, "claude at "+h.Claude+" on "+name, "fix the claude path for "+name) {
		c.claudeVersion(name, string(v))
		status, _ := sh(remote, `"$1" auth status`, claude)
		c.login(name, string(status))
	}
	for _, root := range h.Roots {
		_, err := sh(remote, `[ -d "$1" ]`, expandHome(root, home))
		c.check(err, "root "+root+" exists on "+name, "create it there or fix roots")
	}
	_, err = sh(remote, "command -v tmux >/dev/null")
	c.check(err, "tmux on "+name, "install tmux there")
}
