package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
)

const (
	watcherFresh = 15 * time.Second
	watchRetry   = time.Minute
	watchBackoff = time.Minute
)

type wanted struct {
	To      string `json:"to"`
	PID     int    `json:"pid"`
	Desktop bool   `json:"desktop"`
	Note    string `json:"note"`
}

func magiDir(sub string) (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".magi", sub), nil
}

func wantedPath(sessionID string) (string, error) {
	if !sessionIDPattern.MatchString(sessionID) {
		return "", fmt.Errorf("%q is not a Claude session id", sessionID)
	}
	dir, err := magiDir("wanted")
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, sessionID+".json"), nil
}

func writeWanted(sessionID string, w wanted) error {
	path, err := wantedPath(sessionID)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(w)
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func watchingMachine(name string) (string, error) {
	dir, err := magiDir("watchers")
	if err != nil {
		return "", err
	}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return "", nil
	}
	if err != nil {
		return "", err
	}
	var fresh []string
	for _, e := range entries {
		info, err := e.Info()
		if err == nil && time.Since(info.ModTime()) < watcherFresh {
			fresh = append(fresh, e.Name())
		}
	}
	return matchMachine(fresh, name), nil
}

func waitForPull(input stopInput, out io.Writer) error {
	path, err := wantedPath(input.SessionID)
	if err != nil {
		return err
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	var w wanted
	if err := json.Unmarshal(data, &w); err != nil {
		return err
	}
	w.Note = stillRunning(w.Note, input.BackgroundTasks)
	if err := writeWanted(input.SessionID, w); err != nil {
		return err
	}
	return json.NewEncoder(out).Encode(map[string]string{
		"systemMessage": fmt.Sprintf("Waiting for %s to take this session back.", w.To),
	})
}

func reportWanted(w io.Writer) error {
	dir, err := magiDir("wanted")
	if err != nil {
		return err
	}
	paths, err := filepath.Glob(filepath.Join(dir, "*.json"))
	if err != nil || len(paths) == 0 {
		return err
	}
	fmt.Fprintln(w, "Waiting to be taken back:")
	for _, path := range paths {
		var request wanted
		data, err := os.ReadFile(path)
		if err == nil {
			err = json.Unmarshal(data, &request)
		}
		if err != nil {
			return err
		}
		fmt.Fprintf(w, "  %s  by %s\n", shortID(strings.TrimSuffix(filepath.Base(path), ".json")), request.To)
	}
	fmt.Fprintln(w)
	return nil
}

func watch() error {
	lockPath, err := magiDir("watch.lock")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(lockPath), 0o700); err != nil {
		return err
	}
	lock, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return err
	}
	defer lock.Close()
	if err := syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		return errors.New("magi watch is already running on this machine")
	}
	cfg, _, err := loadConfig()
	if err != nil {
		return err
	}
	local, err := newLocal(context.Background())
	if err != nil {
		return err
	}
	logger := log.New(os.Stderr, "", log.LstdFlags)
	var wg sync.WaitGroup
	for _, name := range cfg.Order {
		wg.Go(func() { watchHost(name, cfg.Hosts[name], local.Name(), logger) })
	}
	wg.Wait()
	return nil
}

func watchHost(name string, host Host, me string, logger *log.Logger) {
	wait := time.Second
	for {
		started := time.Now()
		err := followRequests(context.Background(), name, host, me, logger)
		if time.Since(started) > watchBackoff {
			wait = time.Second
		}
		logger.Printf("%s: watching stopped (%v); trying again in %s", name, err, wait)
		time.Sleep(wait)
		wait = min(wait*2, watchBackoff)
	}
}

func followRequests(ctx context.Context, name string, host Host, me string, logger *log.Logger) error {
	remote, err := newRemote(ctx, host)
	if err != nil {
		return err
	}
	cmd := remote.Command(watchScript, me)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	tried := map[string]time.Time{}
	lines := bufio.NewScanner(stdout)
	for lines.Scan() {
		id := lines.Text()
		if !sessionIDPattern.MatchString(id) || time.Since(tried[id]) < watchRetry {
			continue
		}
		tried[id] = time.Now()
		go take(name, remote, id, logger)
	}
	return cmd.Wait()
}

func take(name string, remote *Remote, id string, logger *log.Logger) {
	data, err := sh(remote, `cat "$HOME/.magi/wanted/$1.json"`, id)
	if err != nil {
		logger.Printf("%s: reading the request for session %s failed: %v", name, shortID(id), err)
		return
	}
	var w wanted
	if err := json.Unmarshal(data, &w); err != nil {
		logger.Printf("%s: the request for session %s is unreadable: %v", name, shortID(id), err)
		return
	}
	logger.Printf("%s: taking session %s back", name, shortID(id))
	pulled, err := pullHere(name, id, w.Note, logger.Writer(), true)
	if err != nil {
		logger.Printf("%s: taking session %s back failed: %v", name, shortID(id), err)
		return
	}
	if _, err := sh(remote, `rm -f "$HOME/.magi/wanted/$1.json"`, id); err != nil {
		logger.Printf("%s: removing the request for session %s failed: %v", name, shortID(id), err)
	}
	logger.Print(pulled.message)
}
