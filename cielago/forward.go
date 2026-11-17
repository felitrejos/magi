package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"
)

const (
	forwardConnected = "connected"
	forwardWaiting   = "waiting"
)

var (
	forwardRetry    = 2 * time.Second
	forwardRetryMax = 30 * time.Second
	forwardSettle   = 3 * time.Second
	forwardLockWait = time.Second
)

type forwardRecord struct {
	Session string    `json:"session"`
	Host    string    `json:"host"`
	SSH     string    `json:"ssh"`
	Rex     *rexVia   `json:"rex,omitempty"`
	Ports   []int     `json:"ports"`
	PID     int       `json:"pid"`
	State   string    `json:"state"`
	Started time.Time `json:"started"`
}

type rexVia struct {
	Server  string `json:"server"`
	Session string `json:"session"`
}

func forwardsDir() (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".cielago", "forwards"), nil
}

func forwardPath(session string) (string, error) {
	if !sessionIDPattern.MatchString(session) {
		return "", fmt.Errorf("%q is not a Claude session id", session)
	}
	dir, err := forwardsDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, session+".json"), nil
}

func lockPath(recordPath string) string {
	return strings.TrimSuffix(recordPath, ".json") + ".lock"
}

func readForward(path string) (forwardRecord, error) {
	var r forwardRecord
	data, err := os.ReadFile(path)
	if err != nil {
		return r, err
	}
	return r, json.Unmarshal(data, &r)
}

func writeForward(path string, r forwardRecord) error {
	data, err := json.Marshal(r)
	if err != nil {
		return err
	}
	tmp := fmt.Sprintf("%s.%d.tmp", path, os.Getpid())
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func runnerAlive(recordPath string) bool {
	f, err := os.Open(lockPath(recordPath))
	if err != nil {
		return false
	}
	defer f.Close()
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_SH|syscall.LOCK_NB); err != nil {
		return errors.Is(err, syscall.EWOULDBLOCK)
	}
	syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
	return false
}

func startForwards(session, host, ssh string, via *rexVia, ports []int) error {
	if err := closeForwards(session); err != nil {
		return err
	}
	path, err := forwardPath(session)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	self, err := os.Executable()
	if err != nil {
		return err
	}
	record := forwardRecord{Session: session, Host: host, SSH: ssh, Rex: via, Ports: ports, State: forwardWaiting, Started: time.Now()}
	if err := writeForward(path, record); err != nil {
		return err
	}
	cmd := exec.Command(self, "forward-run", session)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return errors.Join(err, os.Remove(path))
	}
	record.PID = cmd.Process.Pid
	if err := writeForward(path, record); err != nil {
		return errors.Join(err, syscall.Kill(-record.PID, syscall.SIGTERM), os.Remove(path))
	}
	return cmd.Process.Release()
}

func holdLock(recordPath string) (*os.File, error) {
	f, err := os.OpenFile(lockPath(recordPath), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	deadline := time.Now().Add(forwardLockWait)
	for {
		err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			return f, nil
		}
		if !errors.Is(err, syscall.EWOULDBLOCK) || time.Now().After(deadline) {
			f.Close()
			return nil, err
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func runForwards(session string) error {
	path, err := forwardPath(session)
	if err != nil {
		return err
	}
	lock, err := holdLock(path)
	if errors.Is(err, syscall.EWOULDBLOCK) {
		return nil
	}
	if err != nil {
		return err
	}
	defer lock.Close()
	wait := forwardRetry
	for {
		record, err := readForward(path)
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		program, args := forwardCommand(record)
		tunnel := exec.Command(program, args...)
		if err := tunnel.Start(); err != nil {
			return err
		}
		exited := make(chan struct{})
		go func() {
			tunnel.Wait()
			close(exited)
		}()
		select {
		case <-exited:
		case <-time.After(forwardSettle):
			setForwardState(path, forwardConnected)
			<-exited
			wait = forwardRetry
		}
		setForwardState(path, forwardWaiting)
		time.Sleep(wait)
		wait = min(wait*2, forwardRetryMax)
	}
}

func setForwardState(path, state string) {
	record, err := readForward(path)
	if err != nil || record.State == state {
		return
	}
	record.State = state
	writeForward(path, record)
}

func forwardCommand(r forwardRecord) (string, []string) {
	if r.Rex == nil {
		return "ssh", forwardArgs(r)
	}
	args := []string{"--autostart=false", "-S", r.Rex.Server, "forward", "new", "-s", r.Rex.Session}
	for _, port := range r.Ports {
		p := strconv.Itoa(port)
		args = append(args, "-L", "127.0.0.1:"+p+":localhost:"+p)
	}
	return "rex", args
}

func forwardArgs(r forwardRecord) []string {
	args := []string{
		"-N",
		"-o", "BatchMode=yes",
		"-o", "ConnectTimeout=10",
		"-o", "ControlMaster=no",
		"-o", "ControlPath=none",
		"-o", "ExitOnForwardFailure=yes",
		"-o", "ServerAliveInterval=15",
		"-o", "ServerAliveCountMax=3",
	}
	for _, port := range r.Ports {
		p := strconv.Itoa(port)
		args = append(args, "-L", "127.0.0.1:"+p+":localhost:"+p)
	}
	return append(args, "--", r.SSH)
}

func closeForwards(session string) error {
	path, err := forwardPath(session)
	if err != nil {
		return err
	}
	record, err := readForward(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	alive := runnerAlive(path)
	if err := os.Remove(path); err != nil {
		return err
	}
	if alive && record.PID > 0 {
		if err := syscall.Kill(-record.PID, syscall.SIGTERM); err != nil && !errors.Is(err, syscall.ESRCH) {
			return fmt.Errorf("stopping the forward for session %s: %w", shortID(session), err)
		}
	}
	return nil
}

func listForwards() ([]forwardRecord, error) {
	dir, err := forwardsDir()
	if err != nil {
		return nil, err
	}
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil {
		return nil, err
	}
	var records []forwardRecord
	for _, path := range paths {
		if r, err := readForward(path); err == nil {
			if !runnerAlive(path) {
				r.State = "stopped"
			}
			records = append(records, r)
		}
	}
	sort.Slice(records, func(i, j int) bool { return records[i].Started.Before(records[j].Started) })
	return records, nil
}

func resolveForward(prefix string) (string, error) {
	records, err := listForwards()
	if err != nil {
		return "", err
	}
	var found []string
	for _, r := range records {
		if strings.HasPrefix(r.Session, prefix) {
			found = append(found, r.Session)
		}
	}
	switch len(found) {
	case 1:
		return found[0], nil
	case 0:
		return "", fmt.Errorf("no forward for a session starting with %q", prefix)
	default:
		return "", fmt.Errorf("%q matches %d sessions; give more of the id", prefix, len(found))
	}
}

const closeForwardsScript = `[ -f "$HOME/.cielago/forwards/$1.json" ] || exit 0
for c in "$HOME/.local/bin/cielago" "$HOME/go/bin/cielago" "$(command -v cielago 2>/dev/null)"; do
	if [ -n "$c" ] && [ -x "$c" ]; then
		exec "$c" forwards close "$1"
	fi
done
echo "cielago isn't installed there to stop it" >&2
exit 1`

func closeForwardsOn(m Machine, session string) error {
	if _, err := sh(m, closeForwardsScript, session); err != nil {
		return fmt.Errorf("%s was forwarding this session's ports and still is: %w", m.Name(), err)
	}
	return nil
}

func forwardsCommand(args []string, out io.Writer) error {
	switch {
	case len(args) == 0:
		records, err := listForwards()
		if err != nil {
			return err
		}
		if len(records) == 0 {
			fmt.Fprintln(out, "No ports are forwarded.")
			return nil
		}
		for _, r := range records {
			ports := make([]string, len(r.Ports))
			for i, port := range r.Ports {
				ports[i] = fmt.Sprintf("localhost:%d", port)
			}
			fmt.Fprintf(out, "%s  %s  %s  %s\n", shortID(r.Session), r.Host, strings.Join(ports, ", "), r.State)
		}
		return nil
	case len(args) == 2 && args[0] == "close":
		session, err := resolveForward(args[1])
		if err != nil {
			return err
		}
		return closeForwards(session)
	default:
		return errors.New("usage: cielago forwards [close <session-id>]")
	}
}
