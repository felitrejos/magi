//go:build contract

package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestContractRoundTripToAnotherUser(t *testing.T) {
	dest := os.Getenv("MAGI_CONTRACT_SSH")
	if dest == "" {
		t.Skip("MAGI_CONTRACT_SSH doesn't name an ssh destination to move a session to")
	}
	needClaude(t)
	here, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	host := Host{SSH: dest, Claude: "claude", Roots: []string{"~/developer"}}
	cfg := Config{LocalRoots: []string{here}, Order: []string{"peer"}, Hosts: map[string]Host{"peer": host}}
	path, err := configPath()
	if err != nil {
		t.Fatal(err)
	}
	if err := writeConfig(path, cfg); err != nil {
		t.Fatal(err)
	}
	var out strings.Builder
	if err := tryRoundTrip(cfg, "peer", host, &out); err != nil {
		t.Fatalf("%v\n%s", err, out.String())
	}
}
