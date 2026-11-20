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

func newRemote(ctx context.Context, h Host) (*sh.Remote, error) {
	return sh.NewRemote(ctx, h.SSH, h.Claude)
}

var errNoHost = errors.New("no host")

func loadHost(name string) (Host, error) {
	cfg, path, err := loadConfig()
	if err != nil {
		return Host{}, err
	}
	if _, ok := cfg.Hosts[name]; !ok {
		names := slices.Sorted(maps.Keys(cfg.Hosts))
		return Host{}, fmt.Errorf("%w %q in %s; configured hosts: %s", errNoHost, name, path, strings.Join(names, ", "))
	}
	host := cfg.host(name)
	return host, host.validate(name)
}

func (c Config) host(name string) Host {
	h := c.Hosts[name]
	h.LocalRoots, h.Exclude = c.LocalRoots, c.Exclude
	return h.withDefaults()
}

func (h Host) withDefaults() Host {
	if len(h.Secrets) == 0 {
		h.Secrets = []string{".env", ".env.*"}
	}
	if len(h.Exclude) == 0 {
		h.Exclude = []string{"node_modules", ".venv", "dist", "build", "target"}
	}
	return h
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
	for _, name := range cfg.Order {
		if err := checkHost(name, cfg.Hosts[name].SSH); err != nil {
			return Config{}, path, fmt.Errorf("%s: %w", path, err)
		}
	}
	return cfg, path, nil
}

var hostName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)

func checkHost(name, ssh string) error {
	if !hostName.MatchString(name) {
		return fmt.Errorf("%q isn't a machine name; use letters, digits, dots, dashes and underscores", name)
	}
	if strings.HasPrefix(ssh, "-") {
		return fmt.Errorf("host %q: the ssh destination %q starts with a dash, which ssh reads as an option", name, ssh)
	}
	return nil
}

var bareKey = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

func tomlKey(name string) string {
	if bareKey.MatchString(name) {
		return name
	}
	return `"` + name + `"`
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
