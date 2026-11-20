package main

import (
	"bufio"
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
)

type prompter struct {
	in  *bufio.Reader
	out io.Writer
}

func (p prompter) ask(question, suggestion string) (string, error) {
	if suggestion != "" {
		fmt.Fprintf(p.out, "%s [%s]: ", question, suggestion)
	} else {
		fmt.Fprintf(p.out, "%s: ", question)
	}
	line, err := p.in.ReadString('\n')
	if err != nil && !(errors.Is(err, io.EOF) && line != "") {
		return "", err
	}
	if answer := strings.TrimSpace(line); answer != "" {
		return answer, nil
	}
	return suggestion, nil
}

func list(answer string) []string {
	var items []string
	for _, item := range strings.Split(answer, ",") {
		if item = strings.TrimSpace(item); item != "" {
			items = append(items, item)
		}
	}
	return items
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

var projectFolders = []string{"~/Developer", "~/developer", "~/Projects", "~/projects", "~/code", "~/src", "~/dev"}

func tailscalePeers(ctx context.Context) []string {
	out, err := sh.Exec(ctx, "tailscale", "status", "--json")
	if err != nil {
		return nil
	}
	var status struct {
		Peer map[string]struct {
			HostName string `json:"HostName"`
			Online   bool   `json:"Online"`
		} `json:"Peer"`
	}
	if json.Unmarshal(out, &status) != nil {
		return nil
	}
	var peers []string
	for _, p := range status.Peer {
		if p.Online {
			peers = append(peers, p.HostName)
		}
	}
	return peers
}

func initConfig(in io.Reader, out io.Writer) error {
	p := prompter{in: bufio.NewReader(in), out: out}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
	defer cancel()
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return err
	}
	path, err := configPath()
	if err != nil {
		return err
	}
	cfg, _, err := loadConfig()
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	if cfg.Hosts == nil {
		cfg.Hosts = map[string]Host{}
	}

	fmt.Fprintf(out, "Setting up magi on %s. Press Enter to accept a suggestion.\n\n", local.Name())
	if len(cfg.LocalRoots) == 0 {
		answer, err := p.ask("Where do your projects live on this machine? (comma-separated)", strings.Join(existing(local, projectFolders), ", "))
		if err != nil {
			return err
		}
		if cfg.LocalRoots = list(answer); len(cfg.LocalRoots) == 0 {
			return errors.New("magi needs at least one folder your projects live in here")
		}
	}
	if len(cfg.Exclude) == 0 {
		answer, err := p.ask("Folder names to skip when copying projects and looking for secret files", "node_modules, .venv, dist, build, target")
		if err != nil {
			return err
		}
		cfg.Exclude = list(answer)
	}

	if peers := tailscalePeers(ctx); len(peers) > 0 {
		fmt.Fprintf(out, "Machines online in your tailnet: %s\n", strings.Join(peers, ", "))
	}
	name, err := p.ask("Name for the machine you'll hand sessions to (you'll say \"hand this off to my <name>\")", "")
	if err != nil {
		return err
	}
	if name == "" {
		return errors.New("a name is needed")
	}
	h := cfg.Hosts[name]
	if h.SSH, err = p.ask("ssh destination for "+name, cmp.Or(h.SSH, name)); err != nil {
		return err
	}
	remote, err := newRemote(ctx, h)
	if err != nil {
		return err
	}
	if _, err := sh.Run(remote, "true"); err != nil {
		return fmt.Errorf("ssh %s doesn't work without a password prompt yet; set up key-based ssh (or Tailscale SSH) and run magi init again: %w", h.SSH, err)
	}
	detected, _ := sh.Run(remote, `for t in claude codex; do for c in "$HOME/.local/bin/$t" "$(command -v "$t" 2>/dev/null)" "/opt/homebrew/bin/$t" "/usr/local/bin/$t"; do [ -x "$c" ] && { printf '%s' "$c" | sed "s|^$HOME/|~/|"; break; }; done; echo; done`)
	claude, codex, _ := strings.Cut(strings.TrimSuffix(string(detected), "\n"), "\n")
	if h.Claude, err = p.ask("Path to claude on "+name, cmp.Or(h.Claude, claude)); err != nil {
		return err
	}
	if h.Codex, err = p.ask("Path to codex on "+name+", to move Codex sessions there (blank if you don't use Codex there)", cmp.Or(h.Codex, codex)); err != nil {
		return err
	}
	roots, err := p.ask("Where projects live on "+name+" (comma-separated; new ones go in the first)", cmp.Or(strings.Join(h.Roots, ", "), strings.Join(existing(remote, projectFolders), ", "), "~/projects"))
	if err != nil {
		return err
	}
	h.Roots = list(roots)
	secrets, err := p.ask("Gitignored files to bring along", cmp.Or(strings.Join(h.Secrets, ", "), ".env, .env.*"))
	if err != nil {
		return err
	}
	h.Secrets = list(secrets)

	if err := h.validate(name); err != nil {
		return err
	}
	if _, ok := cfg.Hosts[name]; !ok {
		cfg.Order = append(cfg.Order, name)
	}
	cfg.Hosts[name] = h
	if err := writeConfig(path, cfg); err != nil {
		return err
	}
	fmt.Fprintf(out, "\nSaved %s. Checking everything:\n\n", path)
	return doctor([]string{name}, out)
}
