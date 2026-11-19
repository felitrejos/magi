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

func (c *checks) tested(r testedRange, tool, where, text string) {
	version := versionPattern.FindString(text)
	if err := r.refuse(tool, where, version); err != nil {
		c.fail(err.Error(), "magi refuses to move sessions with it until then")
		return
	}
	c.pass(tool + " " + version + " on " + where + " is a version magi was tested with")
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

func (c *checks) plugin(where, listJSON string) {
	run := func(command string) string {
		if where == "this machine" {
			return command
		}
		return "on " + where + ", run " + command
	}
	var plugins []struct {
		ID      string `json:"id"`
		Enabled bool   `json:"enabled"`
	}
	if err := json.Unmarshal([]byte(listJSON), &plugins); err != nil {
		c.warn("couldn't read claude plugin list --json on "+where+": "+err.Error(), run("claude plugin list")+" to check that the magi plugin is installed and enabled")
		return
	}
	disabled := ""
	for _, p := range plugins {
		if !strings.HasPrefix(p.ID, "magi@") {
			continue
		}
		if p.Enabled {
			c.pass("the magi plugin is installed and enabled on " + where)
			return
		}
		disabled = p.ID
	}
	if disabled != "" {
		c.fail("the magi plugin is installed but disabled on "+where, run("claude plugin enable "+disabled))
		return
	}
	c.fail("the magi plugin isn't installed on "+where, run("claude plugin marketplace add felitrejos/magi && claude plugin install magi@magi"))
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
	versions, err := loadTested()
	if !c.check(err, "read which Claude Code and Codex versions magi was tested with", "check ~/.magi/tested.json") {
		return errors.New("fix the tested versions first")
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
		c.tested(versions.Claude, "Claude Code", "this machine", string(v))
		status, _ := run(ctx, "claude", "auth", "status")
		c.login("this machine", string(status))
		plugins, _ := run(ctx, "claude", "plugin", "list", "--json")
		c.plugin("this machine", string(plugins))
	}
	if v, err := run(ctx, "codex", "--version"); err == nil {
		c.tested(versions.Codex, "Codex", "this machine", string(v))
	}
	running := watchRunning()
	_, tmuxErr := exec.LookPath("tmux")
	switch {
	case running && tmuxErr != nil:
		c.warn("magi watch runs here but tmux isn't installed", "install tmux: a session handed back here opens in a tmux window")
	case running:
		c.pass("magi watch runs here, so sessions on other machines can be handed back")
	default:
		c.warn("magi watch isn't running here", "run magi watch install so a session on another machine can be handed back here")
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
		doctorHost(hostCtx, c, versions, name, h)
		cancelHost()
	}
	if c.failed > 0 {
		return fmt.Errorf("%d check(s) failed", c.failed)
	}
	fmt.Fprintln(out, "\nAll good.")
	return nil
}

func doctorHost(ctx context.Context, c *checks, versions testedVersions, name string, h Host) {
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
		c.tested(versions.Claude, "Claude Code", name, string(v))
		status, _ := sh(remote, `"$1" auth status`, claude)
		c.login(name, string(status))
		plugins, _ := sh(remote, `"$1" plugin list --json`, claude)
		c.plugin(name, string(plugins))
	}
	if _, err := sh(remote, `"${SHELL:-sh}" -lc 'command -v magi' >/dev/null`); err != nil {
		c.fail("magi isn't on PATH on "+name, "the plugin's hooks run it there; install it on "+name+" with curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh")
	} else {
		c.pass("magi is on PATH on " + name)
	}
	if h.Codex != "" {
		if v, err := sh(remote, `"$1" --version`, expandHome(h.Codex, home)); c.check(err, "codex at "+h.Codex+" on "+name, "fix the codex path for "+name) {
			c.tested(versions.Codex, "Codex", name, string(v))
		}
	}
	for _, root := range h.Roots {
		_, err := sh(remote, `[ -d "$1" ]`, expandHome(root, home))
		c.check(err, "root "+root+" exists on "+name, "create it there or fix roots")
	}
	_, err = sh(remote, "command -v tmux >/dev/null")
	c.check(err, "tmux on "+name, "install tmux there")
}
