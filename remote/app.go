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
	"io/fs"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/felitrejos/magi/remote/internal/store"
	"golang.org/x/sys/unix"
)

//go:embed app.requirement
var appRequirement string

var appPath = "/Applications/magi.app"

const (
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

type appCopies struct {
	Copies []string
	Pids   []int
}

func setUpApp(out io.Writer) error {
	if _, ok := parseVersion(version); !ok {
		fmt.Fprintf(out, "This magi is a %s build, so it doesn't install the menu bar app.\n", version)
		return nil
	}
	if macOS, err := unix.Sysctl("kern.osproductversion"); err != nil || !atLeast(macOS, 26) {
		fmt.Fprintf(out, "magi's menu bar app needs macOS 26 or later, and this Mac runs macOS %s, so it isn't installed.\n", macOS)
		return nil
	}
	unlock, err := lockApp()
	if err != nil {
		return err
	}
	defer unlock()
	installed, err := installedApp()
	if err != nil {
		return err
	}
	if newer(installed) {
		fmt.Fprintf(out, "magi's menu bar app %s in %s is newer than this magi (%s), so magi leaves it.\n", installed, appPath, version)
		return nil
	}
	if installed == version && verifyApp(appPath) == nil {
		found, err := findApps()
		if err != nil {
			return err
		}
		if len(found.Pids) == 0 {
			if err := openApp(); err != nil {
				return err
			}
		}
		fmt.Fprintf(out, "magi's menu bar app %s is already installed in %s and running in the menu bar.\n", version, appPath)
		return nil
	}
	if err := installApp(out, true); err != nil {
		return err
	}
	fmt.Fprintln(out, "It's running in the menu bar.")
	if installed == "" {
		fmt.Fprintln(out, "macOS asks once before it brings a Ghostty or Terminal window to the front.")
	}
	return nil
}

func updateApp(logger *log.Logger) bool {
	if _, ok := parseVersion(version); !ok {
		return true
	}
	var b strings.Builder
	err := func() error {
		unlock, err := lockApp()
		if err != nil {
			return err
		}
		defer unlock()
		installed, err := installedApp()
		if err != nil || installed == "" || installed == version || newer(installed) {
			return err
		}
		fmt.Fprintf(&b, "Updating the menu bar app from %s to %s.\n", installed, version)
		return installApp(&b, false)
	}()
	if said := strings.TrimSpace(b.String()); said != "" {
		logger.Print(said)
	}
	if errors.Is(err, errInstalling) {
		logger.Printf("another magi is installing the menu bar app, so the watcher checks it again in an hour")
		return false
	}
	if err != nil {
		logger.Printf("updating the menu bar app to %s failed, so the watcher tries again in an hour: %v", version, err)
		return false
	}
	return true
}

func lockApp() (func(), error) {
	path, err := store.Dir("app.lock")
	if err != nil {
		return nil, err
	}
	return store.Lock(path, errInstalling)
}

var errInstalling = errors.New("another magi is installing the menu bar app right now")

func atLeast(macOS string, major int) bool {
	n, err := strconv.Atoi(strings.SplitN(macOS, ".", 2)[0])
	return err == nil && n >= major
}

func newer(installed string) bool {
	have, ok := parseVersion(installed)
	want, wantOK := parseVersion(version)
	return ok && wantOK && compareVersions(have, want) > 0
}

func installedApp() (string, error) {
	if _, err := os.Lstat(appPath); errors.Is(err, fs.ErrNotExist) {
		return "", nil
	}
	if id, err := appInfo(appPath, "CFBundleIdentifier"); err != nil || id != appBundleID {
		return "", fmt.Errorf("%s isn't magi's menu bar app, so magi leaves it alone; move it out of %s to install magi's", appPath, filepath.Dir(appPath))
	}
	return appInfo(appPath, "CFBundleShortVersionString")
}

func appInfo(app, key string) (string, error) {
	return runQuiet("/usr/bin/plutil", "-extract", key, "raw", "-o", "-", filepath.Join(app, "Contents", "Info.plist"))
}

func appWritable() error {
	dir := filepath.Dir(appPath)
	if unix.Access(dir, unix.W_OK) != nil {
		return fmt.Errorf("installing the menu bar app needs an administrator account: this account can't write to %s", dir)
	}
	if err := unix.Access(appPath, unix.W_OK); err != nil && !errors.Is(err, unix.ENOENT) {
		return fmt.Errorf("this account can't write to %s, so it can't replace it", appPath)
	}
	return nil
}

func installApp(out io.Writer, open bool) error {
	if err := appWritable(); err != nil {
		return err
	}
	stale, _ := filepath.Glob(filepath.Join(os.TempDir(), "magi-app-*"))
	for _, dir := range stale {
		os.RemoveAll(dir)
	}
	work, err := os.MkdirTemp("", "magi-app-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(work)
	archive, err := fetchApp(cmp.Or(os.Getenv("MAGI_RELEASE_URL"), "https://github.com/felitrejos/magi/releases/download/v"+version), work)
	if err != nil {
		return err
	}
	staged, err := unpackApp(archive, filepath.Join(work, "staged"))
	if err != nil {
		return err
	}
	found, err := findApps()
	if err != nil {
		return err
	}
	if err := replaceApp(staged, appPath); err != nil {
		return fmt.Errorf("putting magi.app in %s failed: %w", filepath.Dir(appPath), err)
	}
	fmt.Fprintf(out, "Installed magi's menu bar app %s in %s.\n", version, appPath)
	quitErr := quitApps(found.Pids)
	for _, c := range found.Copies {
		if c == appPath {
			continue
		}
		if _, err := runQuiet(lsregister, "-u", c); err != nil {
			fmt.Fprintf(out, "Unregistering another copy of magi.app, %s, from LaunchServices failed: %v\n", c, err)
			continue
		}
		fmt.Fprintf(out, "Unregistered another copy of magi.app from LaunchServices: %s\n", c)
	}
	if _, err := runQuiet(lsregister, "-f", appPath); err != nil {
		fmt.Fprintf(out, "Registering %s with LaunchServices failed: %v\n", appPath, err)
	}
	if quitErr != nil {
		fmt.Fprintf(out, "The new menu bar app is installed, but the old one didn't quit (%v); quit it from the menu bar to use the new one.\n", quitErr)
	}
	if open || len(found.Pids) > 0 {
		return openApp()
	}
	return nil
}

func findApps() (appCopies, error) {
	var found appCopies
	text, err := runQuiet("/usr/bin/osascript", "-l", "JavaScript", "-e", appCopiesScript, appBundleID)
	if err != nil {
		return found, fmt.Errorf("asking LaunchServices for copies of magi.app failed: %w", err)
	}
	return found, json.Unmarshal([]byte(text), &found)
}

func openApp() error {
	if _, err := runQuiet("/usr/bin/open", appPath); err != nil {
		return fmt.Errorf("opening %s failed: %w", appPath, err)
	}
	return nil
}

func runQuiet(name string, args ...string) (string, error) {
	out, err := exec.Command(name, args...).Output()
	if exit, ok := errors.AsType[*exec.ExitError](err); ok {
		return "", fmt.Errorf("%v: %s", err, strings.TrimSpace(string(exit.Stderr)))
	}
	return strings.TrimSpace(string(out)), err
}

func fetchApp(url, dir string) (string, error) {
	for _, file := range []string{appArchive, "checksums.txt"} {
		if _, err := runQuiet("/usr/bin/curl", "-fsSL", "--connect-timeout", "10", "--max-time", "300", "--proto", "=https,file", "--proto-redir", "=https", "-o", filepath.Join(dir, file), url+"/"+file); err != nil {
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

func unpackApp(archive, dir string) (string, error) {
	if _, err := runQuiet("/usr/bin/ditto", "-x", "-k", archive, dir); err != nil {
		return "", fmt.Errorf("unpacking %s failed: %w", appArchive, err)
	}
	app := filepath.Join(dir, "magi.app")
	if info, err := os.Lstat(app); err != nil || !info.IsDir() {
		return "", fmt.Errorf("%s holds no magi.app folder, so it wasn't installed", appArchive)
	}
	if got, err := appInfo(app, "CFBundleShortVersionString"); err != nil || got != version {
		return "", fmt.Errorf("the downloaded magi.app is version %q, not %s, so it wasn't installed", got, version)
	}
	if err := verifyApp(app); err != nil {
		return "", fmt.Errorf("magi.app isn't signed by magi's certificate, so it wasn't installed: %w", err)
	}
	return app, nil
}

func verifyApp(app string) error {
	_, err := runQuiet("/usr/bin/codesign", "--verify", "--strict", "--deep", "-R", "="+strings.TrimSpace(appRequirement), app)
	return err
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
				return fmt.Errorf("magi.app (pid %d) didn't quit", pid)
			}
			time.Sleep(100 * time.Millisecond)
		}
	}
	return nil
}
