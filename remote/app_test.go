//go:build darwin

package main

import (
	"crypto/sha256"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func fakeApp(t *testing.T, dir, version string) string {
	t.Helper()
	app := filepath.Join(dir, "magi.app")
	if err := os.MkdirAll(filepath.Join(app, "Contents", "MacOS"), 0o755); err != nil {
		t.Fatal(err)
	}
	plist := `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>` + appBundleID + `</string>
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
	if out, err := exec.Command("codesign", "--force", "--sign", "-", app).CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	return app
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

func TestOnlyAnAppSignedByMagisCertificateUnpacks(t *testing.T) {
	dir := t.TempDir()
	app := fakeApp(t, filepath.Join(dir, "build"), "1.0.0")
	archive := filepath.Join(dir, appArchive)
	if out, err := exec.Command("ditto", "-c", "-k", "--keepParent", app, archive).CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	if _, err := unpackApp(archive, filepath.Join(dir, "pinned"), strings.TrimSpace(appRequirement)); err == nil || !strings.Contains(err.Error(), "isn't signed by magi's certificate") {
		t.Fatalf("an ad-hoc signed app passed the pinned requirement: %v", err)
	}
	own, err := exec.Command("codesign", "-d", "-r-", app).Output()
	if err != nil {
		t.Fatal(err)
	}
	_, requirement, _ := strings.Cut(strings.TrimSpace(string(own)), "designated => ")
	got, err := unpackApp(archive, filepath.Join(dir, "own"), requirement)
	if err != nil || got != filepath.Join(dir, "own", "magi.app") {
		t.Fatalf("unpackApp with the app's own requirement = %q, %v", got, err)
	}
}

func TestTheAppIsReplacedInPlace(t *testing.T) {
	dir := t.TempDir()
	for i, dest := range []string{fakeApp(t, filepath.Join(dir, "installed"), "1.0.0"), filepath.Join(dir, "magi.app")} {
		if err := replaceApp(fakeApp(t, filepath.Join(dir, "staged", strconv.Itoa(i)), "2.0.0"), dest); err != nil {
			t.Fatal(err)
		}
		if got, err := appVersion(dest); err != nil || got != "2.0.0" {
			t.Fatalf("after replacing, %s is %q, %v; want 2.0.0", dest, got, err)
		}
	}
}

func TestTheAppFollowsMagisVersion(t *testing.T) {
	was := version
	t.Cleanup(func() { version = was })
	for _, tc := range []struct {
		magi, app string
		stale     bool
	}{
		{"0.10.0", "0.10.0", false},
		{"0.10.0", "0.9.0", true},
		{"0.9.0", "0.10.0", true},
		{"dev", "0.10.0", false},
	} {
		version = tc.magi
		if got := appStale(tc.app); got != tc.stale {
			t.Errorf("magi %s with app %s: stale = %v, want %v", tc.magi, tc.app, got, tc.stale)
		}
	}
}
