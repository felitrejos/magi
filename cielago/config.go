package main

import (
	"errors"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/BurntSushi/toml"
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
	Launcher      string   `toml:"launcher"`
	RexServer     string   `toml:"rex_server"`
	MaxAgents     int      `toml:"max_agents,omitempty"`
	MinFreeMemory string   `toml:"min_free_memory,omitempty"`
	QuietHours    string   `toml:"quiet_hours,omitempty"`
	LocalRoots    []string `toml:"-"`
	Exclude       []string `toml:"-"`
}

func configPath() (string, error) {
	if dir := os.Getenv("XDG_CONFIG_HOME"); dir != "" {
		return filepath.Join(dir, "cielago", "config.toml"), nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".config", "cielago", "config.toml"), nil
}

func loadHost(name string) (Host, error) {
	cfg, path, err := loadConfig()
	if err != nil {
		return Host{}, err
	}
	host, ok := cfg.Hosts[name]
	if !ok {
		names := slices.Sorted(maps.Keys(cfg.Hosts))
		return Host{}, fmt.Errorf("no host %q in %s; configured hosts: %s", name, path, strings.Join(names, ", "))
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
	case h.Launcher == "rex" && h.RexServer == "":
		return missing("rex_server")
	case h.Launcher != "rex" && h.Launcher != "tmux":
		return fmt.Errorf("host %q: launcher must be \"rex\" or \"tmux\", got %q", name, h.Launcher)
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
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), "config-*.toml")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	enc := toml.NewEncoder(f)
	err = enc.Encode(struct {
		LocalRoots []string `toml:"local_roots"`
		Exclude    []string `toml:"exclude"`
	}{cfg.LocalRoots, cfg.Exclude})
	for _, name := range cfg.Order {
		if err == nil {
			_, err = fmt.Fprintf(f, "\n[hosts.%s]\n", tomlKey(name))
		}
		if err == nil {
			err = enc.Encode(cfg.Hosts[name])
		}
	}
	if err := errors.Join(err, f.Close()); err != nil {
		return err
	}
	return os.Rename(f.Name(), path)
}
