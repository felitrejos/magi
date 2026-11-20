package main

import (
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
)

const (
	forwardConnected = "connected"
	forwardWaiting   = "waiting"
)

var (
	forwardRetry    = 2 * time.Second
	forwardRetryMax = 30 * time.Second
	forwardPoll     = 250 * time.Millisecond
	forwardLockWait = time.Second
)

type forwardRecord struct {
	Session string    `json:"session"`
	Host    string    `json:"host"`
	SSH     string    `json:"ssh"`
	Ports   []int     `json:"ports"`
	PID     int       `json:"pid"`
	State   string    `json:"state"`
	Started time.Time `json:"started"`
}

func lockPath(recordPath string) string {
	return strings.TrimSuffix(recordPath, ".json") + ".lock"
}

func readForward(path string) (forwardRecord, error) {
	var r forwardRecord
	return r, store.ReadJSON(path, &r)
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

func startForwards(session, host, ssh string, ports []int) error {
	if err := closeForwards(session); err != nil {
		return err
	}
	path, err := store.SessionPath("forwards", session, ".json")
	if err != nil {
		return err
	}
	self, err := magiLink()
	if err != nil {
		return err
	}
	record := forwardRecord{Session: session, Host: host, SSH: ssh, Ports: ports, State: forwardWaiting, Started: time.Now()}
	if err := store.WriteJSON(path, record); err != nil {
		return err
	}
	cmd := exec.Command(self, "forward-run", session)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return errors.Join(err, os.Remove(path))
	}
	record.PID = cmd.Process.Pid
	if err := store.WriteJSON(path, record); err != nil {
		return errors.Join(err, syscall.Kill(-record.PID, syscall.SIGTERM), os.Remove(path))
	}
	return cmd.Process.Release()
}

func holdLock(recordPath string) (func(), error) {
	deadline := time.Now().Add(forwardLockWait)
	for {
		unlock, err := store.Lock(lockPath(recordPath), syscall.EWOULDBLOCK)
		if !errors.Is(err, syscall.EWOULDBLOCK) || time.Now().After(deadline) {
			return unlock, err
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func runForwards(session string) error {
	path, err := store.SessionPath("forwards", session, ".json")
	if err != nil {
		return err
	}
	unlock, err := holdLock(path)
	if errors.Is(err, syscall.EWOULDBLOCK) {
		return nil
	}
	if err != nil {
		return err
	}
	defer unlock()
	wait := forwardRetry
	for {
		record, err := readForward(path)
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		tunnel := exec.Command("ssh", forwardArgs(record)...)
		if err := tunnel.Start(); err != nil {
			return err
		}
		exited := make(chan error, 1)
		go func() { exited <- tunnel.Wait() }()
		if awaitForward(record, exited) {
			setForwardState(path, forwardConnected)
			<-exited
			wait = forwardRetry
		}
		setForwardState(path, forwardWaiting)
		time.Sleep(wait)
		wait = min(wait*2, forwardRetryMax)
	}
}

func awaitForward(r forwardRecord, exited <-chan error) bool {
	for {
		select {
		case <-exited:
			return false
		case <-time.After(forwardPoll):
			if forwardUp(r) {
				return true
			}
		}
	}
}

func forwardUp(r forwardRecord) bool {
	for _, port := range r.Ports {
		conn, err := net.DialTimeout("tcp", "127.0.0.1:"+strconv.Itoa(port), forwardPoll)
		if err != nil {
			return false
		}
		conn.Close()
	}
	return true
}

func setForwardState(path, state string) {
	record, err := readForward(path)
	if err != nil || record.State == state {
		return
	}
	record.State = state
	store.WriteJSON(path, record)
}

func forwardArgs(r forwardRecord) []string {
	args := localForwards([]string{
		"-N",
		"-o", "BatchMode=yes",
		"-o", "ConnectTimeout=10",
		"-o", "ControlMaster=no",
		"-o", "ControlPath=none",
		"-o", "ExitOnForwardFailure=yes",
		"-o", "ServerAliveInterval=15",
		"-o", "ServerAliveCountMax=3",
	}, r.Ports)
	return append(args, "--", r.SSH)
}

func localForwards(args []string, ports []int) []string {
	for _, port := range ports {
		p := strconv.Itoa(port)
		args = append(args, "-L", "127.0.0.1:"+p+":localhost:"+p)
	}
	return args
}

func closeForwards(session string) error {
	path, err := store.SessionPath("forwards", session, ".json")
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
			return fmt.Errorf("stopping the forward for session %s: %w", store.ShortID(session), err)
		}
	}
	return nil
}

func listForwards() ([]forwardRecord, error) {
	dir, err := store.Dir("forwards")
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

func closeForwardsOn(m sh.Machine, session string) error {
	if _, err := sh.Run(m, closeForwardsScript, session); err != nil {
		return fmt.Errorf("%s was forwarding this session's ports and still is: %w", m.Name(), err)
	}
	return nil
}

func reportForwards(w io.Writer) error {
	records, err := listForwards()
	if err != nil || len(records) == 0 {
		return err
	}
	fmt.Fprintln(w, "Ports forwarded to sessions that moved away; each closes when its session moves on:")
	for _, r := range records {
		ports := make([]string, len(r.Ports))
		for i, port := range r.Ports {
			ports[i] = fmt.Sprintf("localhost:%d", port)
		}
		fmt.Fprintf(w, "  %s on %s  %s  %s\n", store.ShortID(r.Session), r.Host, strings.Join(ports, ", "), r.State)
	}
	fmt.Fprintln(w)
	return nil
}
