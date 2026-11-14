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
  cielago arm <host> <session-id> [-]   hand this session off when its turn ends;
                                        with -, read a note on what was running from stdin
  cielago hook stop                 Claude Code Stop hook (reads the hook JSON on stdin)
`

var sessionIDPattern = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)

type armed struct {
	Host    string `json:"host"`
	PID     int    `json:"pid"`
	Desktop bool   `json:"desktop"`
	Note    string `json:"note"`
}

func main() {
	args := os.Args[1:]
	var err error
	switch {
	case len(args) == 3 && args[0] == "arm":
		err = arm(args[1], args[2], "")
	case len(args) == 4 && args[0] == "arm" && args[3] == "-":
		var note []byte
		if note, err = io.ReadAll(os.Stdin); err == nil {
			err = arm(args[1], args[2], strings.TrimSpace(string(note)))
		}
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

func arm(hostName, sessionID, note string) error {
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
	data, err := json.Marshal(armed{Host: hostName, PID: pid, Desktop: os.Getenv("CLAUDE_CODE_ENTRYPOINT") == "claude-desktop", Note: note})
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

	res, err := handoff(a, input)
	if err != nil {
		return json.NewEncoder(out).Encode(map[string]string{
			"decision": "block",
			"reason":   fmt.Sprintf("cielago could not hand this session off to %s, so it is still running here: %v. Tell the user what failed in plain words.", a.Host, err),
		})
	}
	if err := retire(a, input.TranscriptPath); err != nil {
		return err
	}
	return json.NewEncoder(out).Encode(handedOff(a, res))
}

func handedOff(a armed, res Result) map[string]string {
	where := "It runs with Remote Control, so it's in every Claude app on your account"
	if len(res.ShownOn) > 0 {
		where = fmt.Sprintf("It's open in Rex on %s, and with Remote Control it's in every Claude app on your account", strings.Join(res.ShownOn, ", "))
	}
	if res.RexNote != nil {
		where += fmt.Sprintf(" (%v)", res.RexNote)
	}
	if res.NewFolder != "" {
		where += fmt.Sprintf(". %s didn't have this project yet, so it now lives at %s", a.Host, res.NewFolder)
	}
	if res.ParkNote != nil {
		where += fmt.Sprintf(". Marking this checkout as handed off failed (%v), so handing the session back here will need a clean checkout", res.ParkNote)
	}
	if res.TabNote != nil {
		where += fmt.Sprintf(". The new tab's command didn't start: %v", res.TabNote)
	}
	took := res.Summary()
	if a.Desktop {
		return map[string]string{
			"decision": "block",
			"reason": fmt.Sprintf("cielago handed this session off to %s in %s. %s. "+
				"Tell the user in one or two short lines where the session continues now, "+
				"then archive this session with the archive_session tool and session_id \"self\" (load the tool with ToolSearch if needed). "+
				"Make no other tool calls.", a.Host, took, where),
		}
	}
	return map[string]string{
		"systemMessage": fmt.Sprintf("Handed off to %s in %s. %s. This copy is closing.", a.Host, took, where),
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

func handoff(a armed, input stopInput) (Result, error) {
	host, err := loadHost(a.Host)
	if err != nil {
		return Result{}, err
	}
	return Handoff{
		HostName:       a.Host,
		Note:           a.Note,
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
	if session, block := os.Getenv("REX_SESSION"), os.Getenv("REX_BLOCK"); !a.Desktop && session != "" && block != "" {
		script += " && rex block close -s " + quote(session) + " " + quote(block)
	}
	cmd := exec.Command("sh", "-c", script)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	return cmd.Start()
}
