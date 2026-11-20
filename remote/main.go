package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"runtime/debug"
	"slices"
	"strconv"
	"strings"
	"syscall"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/felitrejos/magi/remote/internal/store"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var version = "dev"

type armed struct {
	Host string `json:"host"`
	PID  int    `json:"pid"`
	Note string `json:"note"`
}

func main() {
	args := os.Args[1:]
	var err error
	switch {
	case len(args) >= 3 && args[0] == "arm":
		err = armCommand(args[1], args[2], args[3:], os.Stdin, os.Stdout)
	case len(args) == 1 && args[0] == "watch":
		err = watch()
	case len(args) == 2 && args[0] == "hook" && args[1] == "stop":
		err = stopHook(os.Stdin, os.Stdout)
	case len(args) == 2 && args[0] == "hook" && args[1] == "prompt":
		err = promptHook(os.Stdin, os.Stdout)
	case len(args) == 2 && args[0] == "forward-run":
		err = runForwards(args[1])
	case len(args) == 3 && args[0] == "forwards" && args[1] == "close":
		err = closeForwards(args[2])
	case len(args) == 1 && args[0] == "mcp":
		err = serveMCP(context.Background(), &mcp.StdioTransport{})
	case len(args) == 2 && args[0] == "wait":
		var text string
		if text, err = waitSession(args[1], waitEvery); err == nil {
			fmt.Println(text)
		}
	case len(args) == 3 && args[0] == "tasks" && args[1] == "wait":
		var r *taskReport
		if r, err = waitTask(args[2], waitEvery); err == nil {
			fmt.Print(describeReport(r))
		}
	case len(args) == 1 && args[0] == "version":
		fmt.Println("magi", currentVersion())
	default:
		fmt.Fprintln(os.Stderr, "magi runs through its Claude Code plugin; ask Claude for what you need")
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "magi:", err)
		os.Exit(1)
	}
}

func currentVersion() string {
	if version != "dev" {
		return version
	}
	if info, ok := debug.ReadBuildInfo(); ok && strings.HasPrefix(info.Main.Version, "v") {
		return strings.TrimPrefix(info.Main.Version, "v")
	}
	return version
}

func armCommand(hostName, sessionID string, flags []string, in io.Reader, out io.Writer) error {
	note, yes := "", false
	for _, flag := range flags {
		switch flag {
		case "--yes":
			yes = true
		case "-":
			b, err := io.ReadAll(in)
			if err != nil {
				return err
			}
			note = strings.TrimSpace(string(b))
		default:
			return fmt.Errorf("magi arm takes a host, a session id, then --yes and - for a note on stdin, not %q", flag)
		}
	}
	return arm(hostName, sessionID, note, yes, out)
}

func arm(hostName, sessionID, note string, yes bool, out io.Writer) error {
	pid, err := strconv.Atoi(os.Getenv("CLAUDE_PID"))
	if err != nil {
		return errors.New("CLAUDE_PID is not set; run arm from inside the Claude Code session being handed off")
	}
	to := hostName
	host, err := loadHost(hostName)
	back := err != nil
	if back {
		if to, err = takesBack(hostName, err); err != nil {
			return err
		}
		if host, err = takerCopies(to); err != nil {
			return err
		}
	}
	local, err := sh.NewLocal(context.Background())
	if err != nil {
		return err
	}
	block, ask, err := describeSession(local, to, host, sessionID, true)
	if err != nil {
		return fmt.Errorf("what goes to %s couldn't be read, so nothing was armed: %w", to, err)
	}
	switch {
	case ask && !yes:
		fmt.Fprintf(out, "Not armed: this copy needs the user's yes first. Show the user what goes, naming the untracked and secret files, and end your turn without arming. After their yes, run magi arm %s %s --yes.\n", hostName, sessionID)
	case back:
		if err := writeWanted(sessionID, wanted{To: to, PID: pid, Note: note}); err != nil {
			return err
		}
		fmt.Fprintf(out, "Asked %s to take this session back: it moves there as soon as this turn ends. End the turn now.\n", to)
	default:
		path, err := store.SessionPath("armed", sessionID, ".json")
		if err != nil {
			return err
		}
		if err := store.WriteJSON(path, armed{Host: hostName, PID: pid, Note: note}); err != nil {
			return err
		}
		fmt.Fprintf(out, "Armed: this session moves to %s as soon as this turn ends. End the turn now.\n", hostName)
	}
	fmt.Fprintln(out, block)
	return nil
}

func takesBack(hostName string, notHost error) (string, error) {
	watching, err := watchers()
	if err != nil {
		return "", errors.Join(notHost, err)
	}
	watcher := matchMachine(watching, hostName)
	if watcher == "" && len(watching) > 0 {
		return "", fmt.Errorf("%w, and no machine called %q takes sessions back from this one; these do: %s", notHost, hostName, strings.Join(watching, ", "))
	}
	if watcher == "" {
		return "", fmt.Errorf("%w, and no machine called %q takes sessions back from this one. That machine isn't set up to take sessions back; the user can set it up from there by asking Claude to set up magi with this machine", notHost, hostName)
	}
	return watcher, nil
}

func matchMachine(names []string, wanted string) string {
	if slices.Contains(names, wanted) {
		return wanted
	}
	var found []string
	for _, name := range names {
		if strings.Contains(strings.ToLower(name), strings.ToLower(wanted)) {
			found = append(found, name)
		}
	}
	if len(found) != 1 {
		return ""
	}
	return found[0]
}

type stopInput struct {
	SessionID       string           `json:"session_id"`
	TranscriptPath  string           `json:"transcript_path"`
	Cwd             string           `json:"cwd"`
	BackgroundTasks []backgroundTask `json:"background_tasks"`
}

type backgroundTask struct {
	Type        string `json:"type"`
	Description string `json:"description"`
	Command     string `json:"command"`
}

func stopHook(in io.Reader, out io.Writer) error {
	var input stopInput
	if err := json.NewDecoder(in).Decode(&input); err != nil {
		return err
	}
	path, err := store.SessionPath("armed", input.SessionID, ".json")
	if err != nil {
		return err
	}
	var a armed
	err = store.ReadJSON(path, &a)
	if errors.Is(err, os.ErrNotExist) {
		return waitForPull(input, out)
	}
	if err := errors.Join(err, os.Remove(path)); err != nil {
		return err
	}
	if os.Getenv("CLAUDE_PID") != strconv.Itoa(a.PID) {
		return nil
	}
	unlock, err := lockMove(input.SessionID)
	if err == nil {
		defer unlock()
		err = recoverBefore(input.SessionID, true)
	}
	if err != nil {
		return block(out, "This handoff didn't start: "+err.Error()+". Tell the user.")
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT, syscall.SIGHUP)
	defer stop()

	res, err := handoff(ctx, a, input)
	if err != nil {
		return block(out, fmt.Sprintf("magi could not hand this session off to %s, so it is still running here: %v. Tell the user what failed in plain words.", a.Host, err))
	}
	return json.NewEncoder(out).Encode(map[string]string{
		"systemMessage": res.Report() + " With Remote Control it's in every Claude app on your account. This copy is closing.",
	})
}

func promptHook(in io.Reader, out io.Writer) error {
	var input stopInput
	if err := json.NewDecoder(in).Decode(&input); err != nil {
		return err
	}
	if !store.SessionIDPattern.MatchString(input.SessionID) {
		return nil
	}
	path, err := store.SessionPath("moved", input.SessionID, "")
	if err != nil {
		return err
	}
	host, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	to := strings.TrimSpace(string(host))
	return block(out, fmt.Sprintf("This session moved to %s, so this copy takes no more messages. Continue it there, or start a new Claude session here and ask it to move session %s here.", to, store.ShortID(input.SessionID)))
}

func block(out io.Writer, reason string) error {
	return json.NewEncoder(out).Encode(map[string]string{"decision": "block", "reason": reason})
}

func handoff(ctx context.Context, a armed, input stopInput) (Result, error) {
	host, err := loadHost(a.Host)
	if err != nil {
		return Result{}, err
	}
	local, err := sh.NewLocal(ctx)
	if err != nil {
		return Result{}, err
	}
	return Move{
		Kind:     "push",
		HostName: a.Host,
		Host:     host,
		Session:  remoteSession{ID: input.SessionID, Transcript: input.TranscriptPath, Cwd: input.Cwd},
		Note:     stillRunning(a.Note, input.BackgroundTasks),
		Retire: func() (int, error) {
			return retire(local, a.Host, input.SessionID, input.TranscriptPath, a.PID)
		},
	}.Run(ctx)
}

func stillRunning(note string, tasks []backgroundTask) string {
	if len(tasks) == 0 {
		return note
	}
	running := make([]string, len(tasks))
	for i, t := range tasks {
		kind := t.Type
		if t.Command != "" {
			kind += " " + t.Command
		}
		running[i] = strings.TrimSpace(t.Description + " (" + kind + ")")
	}
	list := "still running when the turn ended: " + strings.Join(running, "; ")
	if note == "" {
		return list
	}
	return note + "; " + list
}

func retire(local sh.Machine, host, sessionID, transcript string, pid int) (int, error) {
	if err := markMoved(local, sessionID, host); err != nil {
		return 0, err
	}
	journal, err := movePath(sessionID)
	if err != nil {
		return 0, err
	}
	if pid == 0 {
		_, err = sh.Run(local, retireScript, "", transcript, host, sessionID, "", journal)
		return 0, err
	}
	cmd := exec.Command("sh", "-c", retireScript, "magi", strconv.Itoa(pid), transcript, host, sessionID, os.Getenv("TMUX_PANE"), journal)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return 0, err
	}
	return cmd.Process.Pid, cmd.Process.Release()
}
