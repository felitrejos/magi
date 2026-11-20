package main

import (
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestThePluginFetchesAndChecksItsBinary(t *testing.T) {
	dir := t.TempDir()
	home := filepath.Join(dir, "home")
	launcher, err := os.ReadFile(filepath.Join("..", "plugin", "bin", "magi"))
	if err != nil {
		t.Fatal(err)
	}
	release := filepath.Join(dir, "release")
	for _, p := range []string{filepath.Join(dir, "plugin", "bin"), filepath.Join(dir, "plugin", ".claude-plugin"), filepath.Join(dir, "build"), release} {
		if err := os.MkdirAll(p, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "plugin", "bin", "magi"), launcher, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "build", "magi"), []byte("#!/bin/sh\necho fetched \"$@\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	archive := fmt.Sprintf("magi_%s_%s.tar.gz", runtime.GOOS, runtime.GOARCH)
	if out, err := exec.Command("tar", "-czf", filepath.Join(release, archive), "-C", filepath.Join(dir, "build"), "magi").CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	data, err := os.ReadFile(filepath.Join(release, archive))
	if err != nil {
		t.Fatal(err)
	}
	run := func(version, checksums string) (string, error) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(dir, "plugin", ".claude-plugin", "plugin.json"), []byte(`{"name": "magi", "version": "`+version+`"}`), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(release, "checksums.txt"), []byte(checksums), 0o644); err != nil {
			t.Fatal(err)
		}
		cmd := exec.Command(filepath.Join(dir, "plugin", "bin", "magi"), "version")
		cmd.Env = append(os.Environ(), "HOME="+home, "MAGI_RELEASE_URL=file://"+release)
		out, err := cmd.CombinedOutput()
		return string(out), err
	}
	for version, checksums := range map[string]string{"9.9.7": "", "9.9.8": strings.Repeat("0", 64) + "  " + archive + "\n"} {
		if out, err := run(version, checksums); err == nil || !strings.Contains(out, "doesn't match its checksum") {
			t.Fatalf("checksums %q: %q, %v", checksums, out, err)
		}
		if _, err := os.Stat(filepath.Join(home, ".magi", "bin", "magi-"+version)); !os.IsNotExist(err) {
			t.Fatalf("a download with checksums %q was installed", checksums)
		}
	}
	if out, err := run("9.9.9", fmt.Sprintf("%x  %s\n", sha256.Sum256(data), archive)); err != nil || out != "fetched version\n" {
		t.Fatalf("a good checksum: %q, %v", out, err)
	}
	if link, err := os.Readlink(filepath.Join(home, ".magi", "bin", "magi")); err != nil || link != "magi-9.9.9" {
		t.Errorf("the stable link = %q, %v", link, err)
	}
}
