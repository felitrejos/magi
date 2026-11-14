package main

import (
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/BurntSushi/toml"
)

type Config struct {
	LocalRoots []string        `toml:"local_roots"`
	Exclude    []string        `toml:"exclude"`
	Hosts      map[string]Host `toml:"hosts"`
}

type Host struct {
	SSH           string   `toml:"ssh"`
	Claude        string   `toml:"claude"`
	Roots         []string `toml:"roots"`
	Secrets       []string `toml:"secrets"`
	RexServer     string   `toml:"rex_server"`
	NewTabCommand string   `toml:"new_tab_command"`
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
	path, err := configPath()
	if err != nil {
		return Host{}, err
	}
	var cfg Config
	if _, err := toml.DecodeFile(path, &cfg); err != nil {
		return Host{}, fmt.Errorf("read %s: %w", path, err)
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
	case h.RexServer == "":
		return missing("rex_server")
	}
	for _, root := range h.Roots {
		if !strings.HasPrefix(root, "/") && !strings.HasPrefix(root, "~/") {
			return fmt.Errorf("host %q: roots must start with / or ~/, got %q", name, root)
		}
	}
	return nil
}
