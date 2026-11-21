//go:build darwin

package main

import (
	"crypto/sha256"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func init() {
	appPath = "/nonexistent/magi.app"
}

func fakeApp(t *testing.T, dir, id, version string) string {
	t.Helper()
	app := filepath.Join(dir, "magi.app")
	if err := os.MkdirAll(filepath.Join(app, "Contents", "MacOS"), 0o755); err != nil {
		t.Fatal(err)
	}
	plist := `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>` + id + `</string>
<key>CFBundleExecutable</key><string>magi</string>
<key>CFBundleShortVersionString</key><string>` + version + `</string>
</dict></plist>
`
	if err := os.WriteFile(filepath.Join(app, "Contents", "Info.plist"), []byte(plist), 0o644); err != nil {
		t.Fatal(err)
	}
	exe, err := os.ReadFile("/usr/bin/true")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(app, "Contents", "MacOS", "magi"), exe, 0o755); err != nil {
		t.Fatal(err)
	}
	run(t, "/usr/bin/codesign", "--force", "--sign", "-", app)
	return app
}

func run(t *testing.T, name string, args ...string) {
	t.Helper()
	if out, err := exec.Command(name, args...).CombinedOutput(); err != nil {
		t.Fatalf("%s %v: %v: %s", name, args, err, out)
	}
}

func withVersion(t *testing.T, v string) {
	was := version
	version = v
	t.Cleanup(func() { version = was })
}

func withApp(t *testing.T) string {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("TMPDIR", t.TempDir())
	t.Setenv("MAGI_RELEASE_URL", "file://"+t.TempDir())
	dir := t.TempDir()
	was := appPath
	appPath = filepath.Join(dir, "magi.app")
	t.Cleanup(func() { appPath = was })
	return dir
}

func TestTheAppDownloadIsCheckedAgainstItsChecksum(t *testing.T) {
	for _, tc := range []struct {
		name, sums string
		ok         bool
	}{
		{"matching", "%x  magi.app.zip\n%x  magi_darwin_arm64.tar.gz\n", true},
		{"mismatched", "%[2]x  magi.app.zip\n", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			release, work := t.TempDir(), t.TempDir()
			archive := []byte("the app")
			if err := os.WriteFile(filepath.Join(release, appArchive), archive, 0o644); err != nil {
				t.Fatal(err)
			}
			sums := fmt.Sprintf(tc.sums, sha256.Sum256(archive), sha256.Sum256([]byte("another")))
			if err := os.WriteFile(filepath.Join(release, "checksums.txt"), []byte(sums), 0o644); err != nil {
				t.Fatal(err)
			}
			got, err := fetchApp("file://"+release, work)
			if tc.ok && (err != nil || got != filepath.Join(work, appArchive)) {
				t.Fatalf("fetchApp = %q, %v", got, err)
			}
			if !tc.ok && (err == nil || !strings.Contains(err.Error(), "doesn't match its checksum")) {
				t.Fatalf("fetchApp accepted a bad checksum: %q, %v", got, err)
			}
		})
	}
}

func TestOnlyMagisSignedAppAtMagisVersionUnpacks(t *testing.T) {
	withVersion(t, "1.0.0")
	for _, tc := range []struct {
		name, refusal string
		zip           func(t *testing.T, dir string)
	}{
		{"ad-hoc signed", "isn't signed by magi's certificate", func(t *testing.T, dir string) {
			fakeApp(t, dir, appBundleID, "1.0.0")
		}},
		{"an older version", `is version "0.9.0", not 1.0.0`, func(t *testing.T, dir string) {
			fakeApp(t, dir, appBundleID, "0.9.0")
		}},
		{"a symlink", "holds no magi.app folder", func(t *testing.T, dir string) {
			if err := os.Symlink(fakeApp(t, filepath.Join(dir, "elsewhere"), appBundleID, "1.0.0"), filepath.Join(dir, "magi.app")); err != nil {
				t.Fatal(err)
			}
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			build := filepath.Join(dir, "build")
			tc.zip(t, build)
			archive := filepath.Join(dir, appArchive)
			run(t, "/usr/bin/ditto", "-c", "-k", build, archive)
			if _, err := unpackApp(archive, filepath.Join(dir, "staged")); err == nil || !strings.Contains(err.Error(), tc.refusal) {
				t.Fatalf("unpackApp = %v, want %q", err, tc.refusal)
			}
		})
	}
}

func TestTheAppIsReplacedInPlace(t *testing.T) {
	dir := t.TempDir()
	for i, dest := range []string{fakeApp(t, filepath.Join(dir, "installed"), appBundleID, "1.0.0"), filepath.Join(dir, "magi.app")} {
		if err := replaceApp(fakeApp(t, filepath.Join(dir, "staged", strconv.Itoa(i)), appBundleID, "2.0.0"), dest); err != nil {
			t.Fatal(err)
		}
		if got, err := appInfo(dest, "CFBundleShortVersionString"); err != nil || got != "2.0.0" {
			t.Fatalf("after replacing, %s is %q, %v; want 2.0.0", dest, got, err)
		}
	}
}

func TestSetupRefusesAnAppItCantReplace(t *testing.T) {
	withVersion(t, "1.0.0")
	for _, tc := range []struct {
		name, refusal string
		install       func(t *testing.T, dir string)
	}{
		{"another app", "isn't magi's menu bar app", func(t *testing.T, dir string) {
			if err := os.Rename(fakeApp(t, filepath.Join(dir, "other"), "com.example.other", "1.0.0"), filepath.Join(dir, "Magi.app")); err != nil {
				t.Fatal(err)
			}
		}},
		{"a folder this account can't write", "needs an administrator account", func(t *testing.T, dir string) {
			fakeApp(t, dir, appBundleID, "0.9.0")
			readOnly(t, dir)
		}},
		{"an app this account can't write", "can't write to", func(t *testing.T, dir string) {
			readOnly(t, fakeApp(t, dir, appBundleID, "0.9.0"))
		}},
		{"an install in progress", "another magi is installing", func(t *testing.T, _ string) {
			unlock, err := lockApp()
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(unlock)
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := withApp(t)
			tc.install(t, dir)
			if err := setUpApp(io.Discard); err == nil || !strings.Contains(err.Error(), tc.refusal) {
				t.Fatalf("setUpApp = %v, want %q", err, tc.refusal)
			}
			if updateApp(log.New(io.Discard, "", 0)) {
				t.Fatal("the watcher won't try again after a refused update")
			}
		})
	}
}

func readOnly(t *testing.T, path string) {
	if err := os.Chmod(path, 0o555); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Chmod(path, 0o755) })
}

func TestTheWatcherUpdatesOnlyAStaleApp(t *testing.T) {
	for _, tc := range []struct {
		name, magi, app string
		tried           bool
	}{
		{"current", "1.0.0", "1.0.0", false},
		{"absent", "1.0.0", "", false},
		{"dev", "dev", "0.9.0", false},
		{"older", "1.0.0", "0.9.0", true},
		{"newer", "0.9.0", "1.0.0", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			withVersion(t, tc.magi)
			dir := withApp(t)
			if tc.app != "" {
				fakeApp(t, dir, appBundleID, tc.app)
			}
			var logged strings.Builder
			if current := updateApp(log.New(&logged, "", 0)); current == tc.tried || strings.Contains(logged.String(), "failed") != tc.tried {
				t.Fatalf("updateApp = %v, logged %q", current, logged.String())
			}
		})
	}
}
