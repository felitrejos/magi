//go:build darwin

package main

import (
	"cmp"
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"golang.org/x/sys/unix"
)

//go:embed app.requirement
var appRequirement string

const (
	appPath     = "/Applications/magi.app"
	appBundleID = "io.github.felitrejos.magi"
	appArchive  = "magi.app.zip"
	lsregister  = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
)

const appCopiesScript = `function run(argv) {
	ObjC.import("AppKit")
	return JSON.stringify({
		copies: $.NSWorkspace.sharedWorkspace.URLsForApplicationsWithBundleIdentifier(argv[0]).js.map(u => u.path.js),
		pids: $.NSRunningApplication.runningApplicationsWithBundleIdentifier(argv[0]).js.map(a => a.processIdentifier),
	})
}`

func setUpApp(out io.Writer) error {
	if _, err := installApp(out); err != nil {
		return err
	}
	return openApp(out)
}

func updateApp(logger *log.Logger) {
	installed, err := appVersion(appPath)
	if err != nil || !appStale(installed) {
		return
	}
	var b strings.Builder
	running, err := installApp(&b)
	if err == nil && running {
		err = openApp(&b)
	}
	if err != nil {
		logger.Printf("updating the menu bar app from %s to %s failed: %v", installed, version, err)
		return
	}
	logger.Print(strings.TrimSpace(b.String()))
}

func appStale(installed string) bool {
	_, ok := parseVersion(version)
	return ok && installed != version
}

func appVersion(app string) (string, error) {
	out, err := exec.Command("plutil", "-extract", "CFBundleShortVersionString", "raw", "-o", "-", filepath.Join(app, "Contents", "Info.plist")).Output()
	return strings.TrimSpace(string(out)), err
}

func installApp(out io.Writer) (bool, error) {
	work, err := os.MkdirTemp("", "magi-app-")
	if err != nil {
		return false, err
	}
	defer os.RemoveAll(work)
	archive, err := fetchApp(cmp.Or(os.Getenv("MAGI_RELEASE_URL"), "https://github.com/felitrejos/magi/releases/download/v"+version), work)
	if err != nil {
		return false, err
	}
	staged, err := unpackApp(archive, filepath.Join(work, "staged"), strings.TrimSpace(appRequirement))
	if err != nil {
		return false, err
	}
	found, err := exec.Command("osascript", "-l", "JavaScript", "-e", appCopiesScript, appBundleID).Output()
	if err != nil {
		return false, fmt.Errorf("asking LaunchServices for copies of magi.app failed: %w", err)
	}
	var registered struct {
		Copies []string
		Pids   []int
	}
	if err := json.Unmarshal(found, &registered); err != nil {
		return false, err
	}
	if err := quitApps(registered.Pids); err != nil {
		return false, err
	}
	if err := replaceApp(staged, appPath); err != nil {
		return false, fmt.Errorf("putting magi.app in %s failed: %w", filepath.Dir(appPath), err)
	}
	for _, c := range registered.Copies {
		if c == appPath {
			continue
		}
		if err := runQuiet(lsregister, "-u", c); err != nil {
			return false, fmt.Errorf("unregistering %s from LaunchServices failed: %w", c, err)
		}
		fmt.Fprintf(out, "Unregistered another copy of magi.app from LaunchServices: %s\n", c)
	}
	if err := runQuiet(lsregister, "-f", appPath); err != nil {
		return false, fmt.Errorf("registering %s with LaunchServices failed: %w", appPath, err)
	}
	fmt.Fprintf(out, "Installed magi's menu bar app %s in %s.\n", version, appPath)
	return len(registered.Pids) > 0, nil
}

func openApp(out io.Writer) error {
	if err := runQuiet("open", appPath); err != nil {
		return fmt.Errorf("opening %s failed: %w", appPath, err)
	}
	fmt.Fprintln(out, "It's running in the menu bar. macOS asks once before it brings a Ghostty or Terminal window to the front.")
	return nil
}

func runQuiet(name string, args ...string) error {
	if msg, err := exec.Command(name, args...).CombinedOutput(); err != nil {
		return fmt.Errorf("%v: %s", err, strings.TrimSpace(string(msg)))
	}
	return nil
}

func fetchApp(url, dir string) (string, error) {
	for _, file := range []string{appArchive, "checksums.txt"} {
		if err := runQuiet("curl", "-fsSL", "--connect-timeout", "10", "--max-time", "300", "--proto", "=https,file", "--proto-redir", "=https", "-o", filepath.Join(dir, file), url+"/"+file); err != nil {
			return "", fmt.Errorf("downloading %s from %s failed: %w", file, url, err)
		}
	}
	sums, err := os.ReadFile(filepath.Join(dir, "checksums.txt"))
	if err != nil {
		return "", err
	}
	archive := filepath.Join(dir, appArchive)
	data, err := os.ReadFile(archive)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(data)
	for line := range strings.Lines(string(sums)) {
		if f := strings.Fields(line); len(f) == 2 && f[1] == appArchive && f[0] == hex.EncodeToString(sum[:]) {
			return archive, nil
		}
	}
	return "", fmt.Errorf("%s doesn't match its checksum, so it wasn't installed", appArchive)
}

func unpackApp(archive, dir, requirement string) (string, error) {
	if err := runQuiet("ditto", "-x", "-k", archive, dir); err != nil {
		return "", fmt.Errorf("unpacking %s failed: %w", appArchive, err)
	}
	app := filepath.Join(dir, "magi.app")
	if msg, err := exec.Command("codesign", "--verify", "--strict", "--deep", "-R", "="+requirement, app).CombinedOutput(); err != nil {
		return "", fmt.Errorf("magi.app isn't signed by magi's certificate, so it wasn't installed: %s", strings.TrimSpace(string(msg)))
	}
	return app, nil
}

func replaceApp(staged, dest string) error {
	err := unix.RenamexNp(staged, dest, unix.RENAME_SWAP)
	if errors.Is(err, unix.ENOENT) {
		return unix.RenamexNp(staged, dest, unix.RENAME_EXCL)
	}
	return err
}

func quitApps(pids []int) error {
	for _, pid := range pids {
		if err := syscall.Kill(pid, syscall.SIGTERM); err != nil && !errors.Is(err, syscall.ESRCH) {
			return fmt.Errorf("quitting magi.app (pid %d) failed: %w", pid, err)
		}
	}
	deadline := time.Now().Add(10 * time.Second)
	for _, pid := range pids {
		for syscall.Kill(pid, 0) == nil {
			if time.Now().After(deadline) {
				return fmt.Errorf("magi.app (pid %d) didn't quit, so it wasn't replaced", pid)
			}
			time.Sleep(100 * time.Millisecond)
		}
	}
	return nil
}
