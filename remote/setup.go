package main

import (
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

var minGit = [3]int{2, 31, 0}

var projectFolders = []string{"~/Developer", "~/developer", "~/Projects", "~/projects", "~/code", "~/src", "~/dev"}

const findClaudeScript = `for t in claude codex; do for c in "$HOME/.local/bin/$t" "$(command -v "$t" 2>/dev/null)" "/opt/homebrew/bin/$t" "/usr/local/bin/$t"; do [ -x "$c" ] && { printf '%s' "$c" | sed "s|^$HOME/|~/|"; break; }; done; echo; done`

type setupInput struct {
	Host       string   `json:"host" jsonschema:"the name the user calls that machine by, for example homelab; it's how they'll say \"hand this off to my homelab\""`
	SSH        string   `json:"ssh,omitempty" jsonschema:"the ssh destination, such as user@homelab or a Tailscale name; defaults to host, or to what the config already has"`
	Roots      []string `json:"roots,omitempty" jsonschema:"folders projects live in on that machine, each starting with / or ~/; new projects go in the first. Leave out to keep what the config has, or to use the usual folders that exist there, such as ~/developer or ~/projects"`
	LocalRoots []string `json:"local_roots,omitempty" jsonschema:"folders projects live in on this machine. Leave out to keep what the config has, or to use the usual folders that exist here"`
	Codex      bool     `json:"codex,omitempty" jsonschema:"true to move Codex sessions to and from that machine as well"`
}

func setupMachine(in setupInput) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return "", err
	}
	path, err := configPath()
	if err != nil {
		return "", err
	}
	cfg, _, err := loadConfig()
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return "", err
	}
	h := cfg.Hosts[in.Host]
	h.SSH = cmp.Or(in.SSH, h.SSH, in.Host)
	if err := checkHost(in.Host, h.SSH); err != nil {
		return "", err
	}
	remote, err := newRemote(ctx, h)
	if err != nil {
		return "", err
	}
	if _, err := sh.Run(remote, "true"); err != nil {
		return sshFailed(h.SSH, err), nil
	}
	for _, m := range []struct {
		name string
		sh.Machine
	}{{"This machine", local}, {in.Host, remote}} {
		out, err := sh.Run(m, `case $(uname -r) in *[Mm]icrosoft*) uname -r ;; esac`)
		if err != nil {
			return "", err
		}
		if kernel := strings.TrimSpace(string(out)); kernel != "" {
			return fmt.Sprintf("%s runs in WSL (kernel %s), which magi doesn't support. Set magi up between macOS and Linux machines instead.\n", m.name, kernel), nil
		}
	}
	found, err := sh.Run(remote, findClaudeScript)
	if err != nil {
		return "", err
	}
	claude, codex, _ := strings.Cut(strings.TrimSuffix(string(found), "\n"), "\n")
	if h.Claude = cmp.Or(claude, h.Claude); h.Claude == "" {
		return fmt.Sprintf("Claude Code isn't installed on %s. Only the user can install it and log in there. Then call setup_machine again.\n", in.Host), nil
	}
	if in.Codex {
		if h.Codex = cmp.Or(codex, h.Codex); h.Codex == "" {
			return fmt.Sprintf("Codex isn't installed on %s. Install it there, or call setup_machine without codex.\n", in.Host), nil
		}
	}
	if len(in.Roots) > 0 {
		h.Roots = in.Roots
	}
	if len(h.Roots) == 0 {
		if h.Roots = existing(remote, projectFolders); len(h.Roots) == 0 {
			return fmt.Sprintf("None of the usual project folders exist on %s. Ask the user where projects live there, then call setup_machine again with roots.\n", in.Host), nil
		}
	}
	if len(in.LocalRoots) > 0 {
		cfg.LocalRoots = in.LocalRoots
	}
	if len(cfg.LocalRoots) == 0 {
		if cfg.LocalRoots = existing(local, projectFolders); len(cfg.LocalRoots) == 0 {
			return "None of the usual project folders exist on this machine. Ask the user where projects live here, then call setup_machine again with local_roots.\n", nil
		}
	}
	if err := h.validate(in.Host); err != nil {
		return "", err
	}
	if cfg.Hosts == nil {
		cfg.Hosts = map[string]Host{}
	}
	if _, ok := cfg.Hosts[in.Host]; !ok {
		cfg.Order = append(cfg.Order, in.Host)
	}
	cfg.Hosts[in.Host] = h
	if err := writeConfig(path, cfg); err != nil {
		return "", err
	}
	h.LocalRoots, h.Exclude = cfg.LocalRoots, cfg.Exclude
	h = h.withDefaults()
	var b strings.Builder
	fmt.Fprintf(&b, "Saved %s to %s.\n", in.Host, path)
	home, err := homeDir(remote)
	if err != nil {
		return "", err
	}
	plugin, note, pluginErr := installPlugin(remote, expandHome(h.Claude, home))
	fmt.Fprint(&b, note)
	if err := installWatch(&b); err != nil {
		(&checks{out: &b}).fail("magi watch couldn't start here", err.Error())
	}
	if err := doctor(cfg, in.Host, h, plugin, pluginErr, &b); err != nil {
		fmt.Fprintf(&b, "\n%v, so the round trip didn't run. Fix what failed above, then call setup_machine again.\n", err)
		return b.String(), nil
	}
	if err := tryRoundTrip(cfg, in.Host, h, &b); err != nil {
		fmt.Fprintf(&b, "\nThe round trip to %s failed: %v\n", in.Host, err)
	}
	return b.String(), nil
}

func existing(m sh.Machine, candidates []string) []string {
	var found, dirs []string
	for _, c := range candidates {
		dir := strings.TrimPrefix(c, "~/")
		if _, err := sh.Run(m, `d=$HOME/$1; shift; [ -d "$d" ] || exit 1; for f; do [ "$d" -ef "$HOME/$f" ] && exit 1; done; exit 0`, append([]string{dir}, dirs...)...); err == nil {
			found = append(found, c)
			dirs = append(dirs, dir)
		}
	}
	return found
}

type magiPlugin struct {
	ID          string `json:"id"`
	Version     string `json:"version"`
	Enabled     bool   `json:"enabled"`
	InstallPath string `json:"installPath"`
}

func findPlugin(m sh.Machine, claude string) (magiPlugin, error) {
	var plugins []magiPlugin
	if err := runJSON(m, &plugins, `"$1" plugin list --json`, claude); err != nil {
		return magiPlugin{}, err
	}
	for _, p := range plugins {
		if strings.HasPrefix(p.ID, "magi@") {
			return p, nil
		}
	}
	return magiPlugin{}, nil
}

func runJSON(m sh.Machine, into any, script string, args ...string) error {
	out, err := sh.Run(m, script, args...)
	if err != nil {
		return err
	}
	return json.Unmarshal(out, into)
}

func installPlugin(m sh.Machine, claude string) (magiPlugin, string, error) {
	p, err := findPlugin(m, claude)
	if err != nil {
		return p, "", err
	}
	script, did := `"$1" plugin update "$2"`, "Updated"
	switch {
	case p.ID == "":
		var markets []struct{ Name string }
		if err := runJSON(m, &markets, `"$1" plugin marketplace list --json`, claude); err != nil {
			return p, "", err
		}
		script, did = `"$1" plugin install magi@magi`, "Installed"
		if !slices.ContainsFunc(markets, func(market struct{ Name string }) bool { return market.Name == "magi" }) {
			script = `"$1" plugin marketplace add felitrejos/magi && ` + script
		}
	case !p.Enabled:
		script, did = `"$1" plugin enable "$2"`, "Turned on"
	case olderThanThis(p.Version):
		script = `"$1" plugin marketplace update "${2#*@}" && ` + script
	default:
		return p, "", nil
	}
	if _, err := sh.Run(m, script, claude, p.ID); err != nil {
		return p, "", err
	}
	p, err = findPlugin(m, claude)
	return p, fmt.Sprintf("%s the magi plugin on %s.\n", did, m.Name()), err
}

func olderThanThis(installed string) bool {
	there, ok := parseVersion(installed)
	here, ours := parseVersion(currentVersion())
	return ok && ours && compareVersions(there, here) < 0
}

func sshFailed(dest string, err error) string {
	msg := err.Error()
	switch {
	case strings.Contains(msg, "Host key verification failed"):
		return fmt.Sprintf("ssh %s failed: %s\nThis machine doesn't know %s's host key yet. Only the user can check and accept it, by running ssh %s once in a terminal here. Then call setup_machine again.\n", dest, msg, dest, dest)
	case strings.Contains(msg, "Permission denied"):
		return fmt.Sprintf("ssh %s failed: %s\nmagi needs ssh to log in there without a password prompt. Only the user can set that up, with an SSH key or Tailscale SSH. Then call setup_machine again.\n", dest, msg)
	}
	return fmt.Sprintf("ssh %s failed: %s\nCheck that %s is on and reachable from here, then call setup_machine again.\n", dest, msg, dest)
}

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

const checkTimeout = 30 * time.Second

func (c *checks) run(m sh.Machine, script string, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(context.Background(), checkTimeout)
	defer cancel()
	return sh.Run(m.With(ctx), script, args...)
}

func (c *checks) check(err error, what, fix string) bool {
	if err != nil {
		c.fail(what, fmt.Sprintf("%s (%v)", fix, err))
		return false
	}
	c.pass(what)
	return true
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
		c.fail("git isn't installed on "+where, "install git 2.31 or newer there")
		return
	}
	words := append(strings.Fields(text), "", "", "")
	v, ok := parseVersion(words[2])
	switch {
	case !ok:
		c.warn("git's version on "+where+" is unreadable: "+strings.TrimSpace(text), "magi needs git 2.31 or newer.")
	case compareVersions(v, minGit) < 0:
		c.fail("git on "+where+" is "+words[2], "magi needs git 2.31 or newer; update it there")
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
		c.fail("Claude Code isn't logged in on "+where, "only the user can do this: log in to claude.ai there with claude auth login")
		return
	}
	if status.AuthMethod != "claude.ai" {
		c.fail("Claude Code on "+where+" uses "+status.AuthMethod+" authentication", "Remote Control needs a claude.ai login, and only the user can do this: log in to claude.ai there with claude auth login")
		return
	}
	c.pass("logged in to claude.ai on " + where)
}

func (c *checks) plugin(m sh.Machine, where string, p magiPlugin, err error) {
	switch {
	case err != nil:
		c.fail("installing or updating the magi plugin on "+where+" failed", err.Error())
	case p.ID == "":
		c.fail("the magi plugin isn't installed on "+where, "call setup_machine again to install it")
	default:
		v, err := c.run(m, `"$1/bin/magi" version`, p.InstallPath)
		c.check(err, "the magi plugin on "+where+" runs "+strings.TrimSpace(string(v)), "it couldn't fetch magi's binary there")
	}
}

func doctor(cfg Config, name string, h Host, plugin magiPlugin, pluginErr error, out io.Writer) error {
	c := &checks{out: out}
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return err
	}
	fmt.Fprintf(out, "\nThis machine (%s)\n", local.Name())
	versions, err := loadTested()
	if !c.check(err, "read which Claude Code and Codex versions magi was tested with", "check ~/.magi/tested.json") {
		return errors.New("fix the tested versions first")
	}
	_, err = exec.LookPath("ssh")
	c.check(err, "ssh is installed", "install ssh")
	doctorMachine(c, versions, local, "this machine", local.Claude(), local.Codex(), cfg.LocalRoots)
	running := watchRunning()
	_, tmuxErr := exec.LookPath("tmux")
	switch {
	case running && tmuxErr != nil:
		c.warn("magi watch runs here but tmux isn't installed", "install tmux here: a session handed back here opens in a tmux window")
	case running:
		c.pass("magi watch runs here, so sessions on other machines can be handed back")
	default:
		c.warn("magi watch isn't running here", "other machines can't hand sessions back here until it does; its service log says why")
	}
	fmt.Fprintf(out, "\n%s\n", name)
	b, _ := h.budget()
	fmt.Fprintf(out, "  · budget for tasks: %s\n", b)
	if reason := b.noRoom(time.Now(), nil, true); reason != "" {
		fmt.Fprintf(out, "  · no new tasks now: %s\n", reason)
	}
	remote, err := newRemote(context.Background(), h)
	if err != nil {
		return err
	}
	if _, err := c.run(remote, "true"); c.check(err, "ssh "+h.SSH+" works without a password prompt", "only the user can do this: set up key-based SSH to "+h.SSH) {
		doctorMachine(c, versions, remote, name, h.Claude, h.Codex, h.Roots)
		c.plugin(remote, name, plugin, pluginErr)
		_, err = c.run(remote, "command -v tmux >/dev/null")
		c.check(err, "tmux on "+name, "install tmux there; when that needs sudo, only the user can do it")
	}
	if c.failed > 0 {
		return fmt.Errorf("%d check(s) failed", c.failed)
	}
	return nil
}

func doctorMachine(c *checks, versions testedVersions, m sh.Machine, name, claudePath, codexPath string, roots []string) {
	gitOut, gitErr := c.run(m, "git --version")
	c.gitVersion(name, string(gitOut), gitErr)
	_, err := c.run(m, "command -v tar >/dev/null")
	c.check(err, "tar on "+name, "install tar there")
	out, err := c.run(m, `printf '%s' "$HOME"`)
	if err != nil {
		c.fail("read $HOME on "+name, err.Error())
		return
	}
	home := string(out)
	claude := expandHome(claudePath, home)
	if v, err := c.run(m, `"$1" --version`, claude); c.check(err, "claude at "+claudePath+" on "+name, "call setup_machine again so it finds claude there") {
		c.tested(versions.Claude, "Claude Code", name, string(v))
		status, _ := c.run(m, `"$1" auth status`, claude)
		c.login(name, string(status))
	}
	if codexPath != "" {
		if v, err := c.run(m, `"$1" --version`, expandHome(codexPath, home)); c.check(err, "codex at "+codexPath+" on "+name, "call setup_machine again with codex so it finds codex there") {
			c.tested(versions.Codex, "Codex", name, string(v))
		}
	}
	for _, root := range roots {
		_, err := c.run(m, `[ -d "$1" ]`, expandHome(root, home))
		c.check(err, "folder "+root+" exists on "+name, "create it there, or call setup_machine with folders that exist")
	}
}

var tryProject = regexp.MustCompile(`^magi-try-[0-9a-f]{8}$`)

var tryWords = []string{"HERON", "OTTER", "BADGER", "KESTREL", "MARMOT", "IBIS"}

type claudeReply struct {
	SessionID string `json:"session_id"`
	Result    string `json:"result"`
	IsError   bool   `json:"is_error"`
}

func claudeEnv() []string {
	var env []string
	for _, kv := range os.Environ() {
		if strings.HasPrefix(kv, "CLAUDE_CODE_") || strings.HasPrefix(kv, "CLAUDECODE=") {
			continue
		}
		env = append(env, kv)
	}
	return env
}

func tryRoundTrip(cfg Config, name string, host Host, out io.Writer) (err error) {
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return err
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	project := "magi-try-" + store.NewTaskID()
	dir := filepath.Join(expandHome(cfg.LocalRoots[0], home), project)
	word := tryWords[rand.IntN(len(tryWords))]
	fmt.Fprintf(out, "\nTrying a round trip to %s with a throwaway session in %s. It takes three short Claude turns on the user's account.\n", name, dir)
	var id string
	defer func() { err = errors.Join(err, tryCleanup(local, cfg.LocalRoots, host, project, id)) }()
	if _, err := sh.Run(local, tryScript, "setup", dir); err != nil {
		return fmt.Errorf("creating the test project failed: %w", err)
	}
	started, err := tryClaude(dir, "Remember the word "+word+". Reply with just: ok")
	if err != nil {
		return fmt.Errorf("starting a Claude session here failed: %w", err)
	}
	id = started.SessionID
	if err := (mover{session: id, host: name, out: io.Discard}).run(); err != nil {
		return fmt.Errorf("moving the session to %s failed: %w", name, err)
	}
	if _, err := pullHere(name, id, "", out, false); err != nil {
		return fmt.Errorf("bringing the session back from %s failed: %w", name, err)
	}
	checked, err := tryClaude(dir, "--resume", id, "What word did I ask you to remember? Reply with just the word.")
	if err != nil {
		return fmt.Errorf("resuming the session here failed: %w", err)
	}
	if !strings.Contains(strings.ToUpper(checked.Result), word) {
		return fmt.Errorf("the session came back but didn't remember %s; it said %q", word, checked.Result)
	}
	if notes, err := os.ReadFile(filepath.Join(dir, "notes.txt")); err != nil || string(notes) != "base\nuncommitted line\n" {
		return fmt.Errorf("the session came back, but its uncommitted change didn't: notes.txt is %q (%v)", notes, err)
	}
	fmt.Fprintf(out, "magi works with %s: the session went there and back with its uncommitted change, and still remembered %s.\n", name, word)
	return nil
}

func tryClaude(dir string, args ...string) (claudeReply, error) {
	cmd := exec.Command("claude", append([]string{"-p", "--model", "haiku", "--output-format", "json"}, args...)...)
	cmd.Dir, cmd.Env = dir, claudeEnv()
	data, err := cmd.Output()
	var reply claudeReply
	if err == nil {
		err = json.Unmarshal(data, &reply)
	}
	if err == nil && (reply.IsError || reply.SessionID == "") {
		err = fmt.Errorf("claude answered %q", reply.Result)
	}
	return reply, err
}

func tryCleanup(local *sh.Local, localRoots []string, host Host, project, sessionID string) error {
	if !tryProject.MatchString(project) {
		return fmt.Errorf("refusing to remove %q, which isn't a magi try project", project)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	var errs []error
	if remote, err := newRemote(ctx, host); err != nil {
		errs = append(errs, err)
	} else {
		if sessionID != "" {
			errs = append(errs, closeRecordedTab(remote, sessionID))
		}
		_, err := sh.Run(remote, tryScript, append([]string{"cleanup", project, sessionID}, host.Roots...)...)
		errs = append(errs, err)
	}
	_, err := sh.Run(local, tryScript, append([]string{"cleanup", project, sessionID}, localRoots...)...)
	if err := errors.Join(append(errs, err)...); err != nil {
		return fmt.Errorf("removing the test project %s failed: %w", project, err)
	}
	return nil
}
