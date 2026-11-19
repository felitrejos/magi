package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/BurntSushi/toml"
	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

type Config struct {
	LocalRoots []string        `toml:"local_roots"`
	Exclude    []string        `toml:"exclude"`
	Hosts      map[string]Host `toml:"hosts"`
	Order      []string        `toml:"-"`
}

type Host struct {
	SSH           string   `toml:"ssh"`
	Claude        string   `toml:"claude"`
	Codex         string   `toml:"codex,omitempty"`
	Roots         []string `toml:"roots"`
	Secrets       []string `toml:"secrets"`
	MaxAgents     int      `toml:"max_agents,omitempty"`
	MinFreeMemory string   `toml:"min_free_memory,omitempty"`
	QuietHours    string   `toml:"quiet_hours,omitempty"`
	LocalRoots    []string `toml:"-"`
	Exclude       []string `toml:"-"`
}

func configPath() (string, error) {
	if dir := os.Getenv("XDG_CONFIG_HOME"); dir != "" {
		return filepath.Join(dir, "magi", "config.toml"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".config", "magi", "config.toml"), nil
}

func (h Host) program(codex bool) string {
	if codex {
		return h.Codex
	}
	return h.Claude
}

func newRemote(ctx context.Context, h Host) (*sh.Remote, error) {
	return sh.NewRemote(ctx, h.SSH, h.Claude, h.Codex)
}

var errNoHost = errors.New("no host")

func loadHost(name string) (Host, error) {
	cfg, path, err := loadConfig()
	if err != nil {
		return Host{}, err
	}
	host, ok := cfg.Hosts[name]
	if !ok {
		names := slices.Sorted(maps.Keys(cfg.Hosts))
		return Host{}, fmt.Errorf("%w %q in %s; configured hosts: %s", errNoHost, name, path, strings.Join(names, ", "))
	}
	host.LocalRoots = cfg.LocalRoots
	host.Exclude = cfg.Exclude
	return host, host.validate(name)
}

func (h Host) validate(name string) error {
	missing := func(field string) error {
		return fmt.Errorf("host %q: %s is required", name, field)
	}
	switch {
	case h.SSH == "":
		return missing("ssh")
	case h.Claude == "":
		return missing("claude")
	case len(h.Roots) == 0:
		return missing("roots")
	}
	for _, root := range h.Roots {
		if !strings.HasPrefix(root, "/") && !strings.HasPrefix(root, "~/") {
			return fmt.Errorf("host %q: roots must start with / or ~/, got %q", name, root)
		}
	}
	if _, err := h.budget(); err != nil {
		return fmt.Errorf("host %q: %w", name, err)
	}
	for _, e := range h.Exclude {
		if e == "" || strings.Contains(e, "/") {
			return fmt.Errorf("exclude: %q isn't a file or folder name; list names such as \"node_modules\", without slashes", e)
		}
	}
	return nil
}

func loadConfig() (Config, string, error) {
	path, err := configPath()
	if err != nil {
		return Config{}, "", err
	}
	var cfg Config
	md, err := toml.DecodeFile(path, &cfg)
	if err != nil {
		return Config{}, path, fmt.Errorf("read %s: %w", path, err)
	}
	for _, key := range md.Keys() {
		if len(key) >= 2 && key[0] == "hosts" && !slices.Contains(cfg.Order, key[1]) {
			cfg.Order = append(cfg.Order, key[1])
		}
	}
	for _, name := range slices.Sorted(maps.Keys(cfg.Hosts)) {
		if !slices.Contains(cfg.Order, name) {
			cfg.Order = append(cfg.Order, name)
		}
	}
	return cfg, path, nil
}

var bareKey = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

func tomlKey(name string) string {
	if bareKey.MatchString(name) {
		return name
	}
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range name {
		switch {
		case r == '"' || r == '\\':
			b.WriteByte('\\')
			b.WriteRune(r)
		case r < 0x20 || r == 0x7f:
			fmt.Fprintf(&b, "\\u%04X", r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
	return b.String()
}

func writeConfig(path string, cfg Config) error {
	var b bytes.Buffer
	enc := toml.NewEncoder(&b)
	err := enc.Encode(struct {
		LocalRoots []string `toml:"local_roots"`
		Exclude    []string `toml:"exclude"`
	}{cfg.LocalRoots, cfg.Exclude})
	for _, name := range cfg.Order {
		if err == nil {
			fmt.Fprintf(&b, "\n[hosts.%s]\n", tomlKey(name))
			err = enc.Encode(cfg.Hosts[name])
		}
	}
	if err != nil {
		return err
	}
	return store.WriteAtomic(path, b.Bytes())
}
