package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"syscall"
)

const usage = `cielago moves a Claude Code session to another machine.

Usage:
  cielago arm <host> <session-id>   hand this session off when its turn ends
  cielago hook stop                 Claude Code Stop hook (reads the hook JSON on stdin)
`

var sessionIDPattern = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)

type armed struct {
	Host    string `json:"host"`
	PID     int    `json:"pid"`
	Desktop bool   `json:"desktop"`
}

func main() {
	args := os.Args[1:]
	var err error
	switch {
	case len(args) == 3 && args[0] == "arm":
		err = arm(args[1], args[2])
	case len(args) == 2 && args[0] == "hook" && args[1] == "stop":
		err = stopHook(os.Stdin, os.Stdout)
	default:
		fmt.Fprint(os.Stderr, usage)
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "cielago:", err)
		os.Exit(1)
	}
}

func armedPath(sessionID string) (string, error) {
	if !sessionIDPattern.MatchString(sessionID) {
		return "", fmt.Errorf("%q is not a Claude session id", sessionID)
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".cielago", "armed", sessionID+".json"), nil
}

func arm(hostName, sessionID string) error {
	if _, err := loadHost(hostName); err != nil {
		return err
	}
	pid, err := strconv.Atoi(os.Getenv("CLAUDE_PID"))
	if err != nil {
		return errors.New("CLAUDE_PID is not set; run arm from inside the Claude Code session being handed off")
	}
	path, err := armedPath(sessionID)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(armed{Host: hostName, PID: pid, Desktop: os.Getenv("CLAUDE_CODE_ENTRYPOINT") == "claude-desktop"})
	if err != nil {
		return err
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		return err
	}
	fmt.Printf("Armed: this session moves to %s as soon as this turn ends. End the turn now.\n", hostName)
	return nil
}

type stopInput struct {
	SessionID      string `json:"session_id"`
	TranscriptPath string `json:"transcript_path"`
	Cwd            string `json:"cwd"`
}

func stopHook(in io.Reader, out io.Writer) error {
	var input stopInput
	if err := json.NewDecoder(in).Decode(&input); err != nil {
		return err
	}
	path, err := armedPath(input.SessionID)
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
	if err := os.Remove(path); err != nil {
		return err
	}
	var a armed
	if err := json.Unmarshal(data, &a); err != nil {
		return err
	}
	ours, err := isAncestor(a.PID)
	if err != nil || !ours {
		return err
	}

	steps, err := handoff(a.Host, input)
	if err != nil {
		return json.NewEncoder(out).Encode(map[string]string{
			"decision": "block",
			"reason":   fmt.Sprintf("cielago could not hand this session off to %s, so it is still running here: %v. Tell the user what failed in plain words.", a.Host, err),
		})
	}
	if err := retire(a, input.TranscriptPath); err != nil {
		return err
	}
	return json.NewEncoder(out).Encode(handedOff(a, summarize(steps)))
}

func handedOff(a armed, took string) map[string]string {
	if a.Desktop {
		return map[string]string{
			"decision": "block",
			"reason": fmt.Sprintf("cielago handed this session off to %s in %s; it is now running there with Remote Control. "+
				"Tell the user in one short line that the session continues on %s and that they can open it from the Claude app, "+
				"then archive this session with the archive_session tool and session_id \"self\" (load the tool with ToolSearch if needed). "+
				"Make no other tool calls.", a.Host, took, a.Host),
		}
	}
	return map[string]string{
		"systemMessage": fmt.Sprintf("Handed off to %s in %s. Open it from the Claude app; this copy is closing.", a.Host, took),
	}
}

func isAncestor(pid int) (bool, error) {
	for p := os.Getppid(); p > 1; {
		if p == pid {
			return true, nil
		}
		out, err := run(context.Background(), "", nil, "ps", "-o", "ppid=", "-p", strconv.Itoa(p))
		if err != nil {
			return false, err
		}
		if p, err = strconv.Atoi(strings.TrimSpace(string(out))); err != nil {
			return false, err
		}
	}
	return false, nil
}

func handoff(hostName string, input stopInput) ([]Step, error) {
	host, err := loadHost(hostName)
	if err != nil {
		return nil, err
	}
	return Handoff{
		HostName:       hostName,
		Host:           host,
		SessionID:      input.SessionID,
		TranscriptPath: input.TranscriptPath,
		Cwd:            input.Cwd,
	}.Run()
}

func retire(a armed, transcriptPath string) error {
	p := strconv.Itoa(a.PID)
	stop := "sleep 1; kill -TERM " + p + " && "
	if a.Desktop {
		stop = ""
	}
	script := stop + "while kill -0 " + p + " 2>/dev/null; do sleep 0.2; done && mv -- " +
		quote(transcriptPath) + " " + quote(transcriptPath+".handed-off-to-"+a.Host)
	cmd := exec.Command("sh", "-c", script)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	return cmd.Start()
}
