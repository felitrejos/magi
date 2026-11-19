package main

import (
	"fmt"
	"html"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
)

const watchService = "magi-watch"

func watchServicePath() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	if runtime.GOOS == "darwin" {
		return filepath.Join(home, "Library", "LaunchAgents", watchService+".plist"), nil
	}
	return filepath.Join(home, ".config", "systemd", "user", watchService+".service"), nil
}

func launchdDomain() string {
	return "gui/" + strconv.Itoa(os.Getuid())
}

func installWatch(out io.Writer) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	if exe, err = filepath.EvalSymlinks(exe); err != nil {
		return err
	}
	path, err := watchServicePath()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	if runtime.GOOS == "darwin" {
		home, err := os.UserHomeDir()
		if err != nil {
			return err
		}
		logPath := filepath.Join(home, "Library", "Logs", watchService+".log")
		if err := os.WriteFile(path, []byte(launchdPlist(exe, os.Getenv("PATH"), logPath)), 0o644); err != nil {
			return err
		}
		exec.Command("launchctl", "bootout", launchdDomain()+"/"+watchService).Run()
		if msg, err := exec.Command("launchctl", "bootstrap", launchdDomain(), path).CombinedOutput(); err != nil {
			return fmt.Errorf("launchctl bootstrap: %v: %s", err, strings.TrimSpace(string(msg)))
		}
		fmt.Fprintf(out, "magi watch runs in the background and starts at login.\n  service: %s\n  log:     %s\n", path, logPath)
		return nil
	}
	if err := os.WriteFile(path, []byte(systemdUnit(exe, os.Getenv("PATH"))), 0o644); err != nil {
		return err
	}
	for _, args := range [][]string{{"daemon-reload"}, {"enable", watchService + ".service"}, {"restart", watchService + ".service"}} {
		if msg, err := exec.Command("systemctl", append([]string{"--user"}, args...)...).CombinedOutput(); err != nil {
			return fmt.Errorf("systemctl --user %s: %v: %s", strings.Join(args, " "), err, strings.TrimSpace(string(msg)))
		}
	}
	fmt.Fprintf(out, "magi watch runs in the background with your user session.\n  service: %s\n  log:     journalctl --user -u %s\n", path, watchService)
	return nil
}

func uninstallWatch(out io.Writer) error {
	path, err := watchServicePath()
	if err != nil {
		return err
	}
	if runtime.GOOS == "darwin" {
		exec.Command("launchctl", "bootout", launchdDomain()+"/"+watchService).Run()
	} else {
		exec.Command("systemctl", "--user", "disable", "--now", watchService+".service").Run()
	}
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return err
	}
	if runtime.GOOS != "darwin" {
		exec.Command("systemctl", "--user", "daemon-reload").Run()
	}
	fmt.Fprintln(out, "magi watch no longer runs in the background.")
	return nil
}

func watchRunning() bool {
	if runtime.GOOS == "darwin" {
		return exec.Command("launchctl", "print", launchdDomain()+"/"+watchService).Run() == nil
	}
	return exec.Command("systemctl", "--user", "is-active", "--quiet", watchService+".service").Run() == nil
}

func launchdPlist(exe, path, logPath string) string {
	return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>` + watchService + `</string>
  <key>ProgramArguments</key>
  <array>
    <string>` + html.EscapeString(exe) + `</string>
    <string>watch</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>` + html.EscapeString(path) + `</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>` + html.EscapeString(logPath) + `</string>
</dict>
</plist>
`
}

func systemdUnit(exe, path string) string {
	return "[Unit]\nDescription=magi watch\nAfter=network-online.target\n\n[Service]\nExecStart=" + strconv.Quote(exe) + " watch\nEnvironment=" + strconv.Quote("PATH="+path) + "\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n"
}
