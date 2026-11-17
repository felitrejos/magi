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
	"path/filepath"
	"regexp"
	"runtime/debug"
	"strconv"
	"strings"
	"syscall"
	"time"
)

var version = "dev"

const usage = `cielago moves a Claude Code or Codex session to another machine, or sends Claude work there.

Usage:
  cielago init                          set up this machine and a machine to hand off to
  cielago doctor [host...]              check that everything a handoff needs is in place
  cielago pull <host> [session-id]      bring a Claude Code or Codex session back from <host>
                                        and continue it here
  cielago move <session-id> <host> [--json] [--stay]
                                        move a Claude Code or Codex session on this machine to
                                        <host>, from outside it: now if it's idle, or when its
                                        turn ends; Codex needs codex = "<path>" for <host>
  cielago status                        recent handoffs and parked checkouts
  cielago unpark [dir]                  stop treating a checkout as parked
  cielago allow [dir]                   let the project's .cielago.toml run after handoffs
  cielago open <folder> [--codex] [--model <model>] [--json]
                                        start a Claude (or Codex) session there, in a Rex session
                                        named after its folder; --resume <id> continues a session
                                        as a copy; --chat starts one in ~/Chats instead;
                                        --list prints the project folders it knows
  cielago forwards                      ports this machine forwards to sessions that moved
  cielago forwards close <session-id>   stop forwarding a moved session's ports
  cielago tasks                         work sent to other machines, and its state
  cielago tasks wait <id>               wait until a task finishes, then print how it ended
  cielago tasks show <id>               a task's output; collects it if it's finished
  cielago tasks cancel <id>             stop a task and remove its copy
  cielago tasks clean                   collect finished tasks, drop day-idle copies and week-old tasks
  cielago version

Used by the Claude Code plugin:
  cielago arm <host> <session-id> [-]   hand this session off when its turn ends;
                                        with -, read a note on what was running from stdin
  cielago hook stop                     the plugin's Stop hook (reads the hook JSON on stdin)
  cielago mcp                           the plugin's tool server, on stdin and stdout
  cielago forward-run <session-id>      keeps a moved session's ports forwarded (started by a move)
`

var sessionIDPattern = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)

type armed struct {
	Host    string `json:"host"`
	PID     int    `json:"pid"`
	Desktop bool   `json:"desktop"`
	Note    string `json:"note"`
	Notify  bool   `json:"notify,omitempty"`
	Stay    bool   `json:"stay,omitempty"`
}

func main() {
	args := os.Args[1:]
	var err error
	switch {
	case len(args) == 3 && args[0] == "arm":
		err = arm(args[1], args[2], "", os.Stdout)
	case len(args) == 4 && args[0] == "arm" && args[3] == "-":
		var note []byte
		if note, err = io.ReadAll(os.Stdin); err == nil {
			err = arm(args[1], args[2], strings.TrimSpace(string(note)), os.Stdout)
		}
	case len(args) == 2 && args[0] == "hook" && args[1] == "stop":
		err = stopHook(os.Stdin, os.Stdout)
	case len(args) == 1 && args[0] == "init":
		err = initConfig(os.Stdin, os.Stdout)
	case len(args) >= 1 && args[0] == "doctor":
		err = doctor(args[1:], os.Stdout)
	case len(args) == 2 && args[0] == "pull":
		err = pullCommand(args[1], "")
	case len(args) == 3 && args[0] == "pull":
		err = pullCommand(args[1], args[2])
	case len(args) <= 2 && len(args) >= 1 && args[0] == "allow":
		dir := "."
		if len(args) == 2 {
			dir = args[1]
		}
		err = allowProject(dir, os.Stdin, os.Stdout)
	case len(args) == 1 && args[0] == "status":
		err = status(os.Stdout)
	case len(args) <= 2 && len(args) >= 1 && args[0] == "unpark":
		dir := "."
		if len(args) == 2 {
			dir = args[1]
		}
		err = unpark(dir, os.Stdout)
	case len(args) >= 1 && args[0] == "open":
		err = openCommand(args[1:], os.Stdout)
	case len(args) >= 1 && args[0] == "forwards":
		err = forwardsCommand(args[1:], os.Stdout)
	case len(args) == 2 && args[0] == "forward-run":
		err = runForwards(args[1])
	case len(args) >= 1 && args[0] == "move":
		err = moveCommand(args[1:], os.Stdout)
	case len(args) == 1 && args[0] == "mcp":
		err = serveMCP(context.Background())
	case len(args) == 1 && args[0] == "tasks":
		err = printTasks(os.Stdout)
	case len(args) == 2 && args[0] == "tasks" && args[1] == "clean":
		err = cleanTasks(os.Stdout)
	case len(args) == 3 && args[0] == "tasks" && args[1] == "wait":
		var r *taskReport
		if r, err = waitTask(args[2], waitEvery); err == nil {
			fmt.Print(describeReport(r))
		}
	case len(args) == 3 && args[0] == "tasks" && (args[1] == "show" || args[1] == "cancel"):
		var r *taskReport
		if r, err = collectTask(args[2], args[1] == "cancel"); err == nil {
			fmt.Print(describeReport(r))
		}
	case len(args) == 1 && (args[0] == "version" || args[0] == "--version"):
		fmt.Println("cielago", currentVersion())
	default:
		fmt.Fprint(os.Stderr, usage)
		os.Exit(2)
	}
	if errors.Is(err, errReported) {
		os.Exit(1)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "cielago:", err)
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

func armedPath(sessionID, suffix string) (string, error) {
	if !sessionIDPattern.MatchString(sessionID) {
		return "", fmt.Errorf("%q is not a Claude session id", sessionID)
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".cielago", "armed", sessionID+suffix), nil
}

func writeArmed(path string, a armed, flag int) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(a)
	if err != nil {
		return err
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|flag, 0o600)
	if err != nil {
		return err
	}
	_, err = f.Write(data)
	return errors.Join(err, f.Close())
}

func arm(hostName, sessionID, note string, out io.Writer) error {
	if _, err := loadHost(hostName); err != nil {
		return err
	}
	pid, err := strconv.Atoi(os.Getenv("CLAUDE_PID"))
	if err != nil {
		return errors.New("CLAUDE_PID is not set; run arm from inside the Claude Code session being handed off")
	}
	path, err := armedPath(sessionID, ".json")
	if err != nil {
		return err
	}
	if err := writeArmed(path, armed{Host: hostName, PID: pid, Desktop: os.Getenv("CLAUDE_CODE_ENTRYPOINT") == "claude-desktop", Note: note}, os.O_TRUNC); err != nil {
		return err
	}
	fmt.Fprintf(out, "Armed: this session moves to %s as soon as this turn ends. End the turn now.\n", hostName)
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
	path, err := armedPath(input.SessionID, ".json")
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
		if a.Notify {
			err = errors.Join(err, writeMoveResult(input.SessionID, moveReport{Event: "failed", Message: "the session that ended its turn wasn't the one armed to move"}))
		}
		return err
	}

	res, err := handoff(a, input)
	if err != nil {
		reason := fmt.Sprintf("cielago could not hand this session off to %s, so it is still running here: %v.", a.Host, err)
		if a.Notify {
			if writeErr := writeMoveResult(input.SessionID, moveReport{Event: "failed", Message: fmt.Sprintf("the handoff to %s failed, so the session keeps running here: %v", a.Host, err)}); writeErr != nil {
				reason += fmt.Sprintf(" Telling cielago move about it also failed: %v.", writeErr)
			}
		}
		return json.NewEncoder(out).Encode(map[string]string{
			"decision": "block",
			"reason":   reason + " Tell the user what failed in plain words.",
		})
	}
	if a.Notify {
		res.note(writeMoveResult(input.SessionID, moveReport{Event: "moved", OK: true, Message: fmt.Sprintf("Moved session %s to %s in %s. It's running %s.", shortID(input.SessionID), a.Host, res.Summary(), res.Where), ToDir: res.ToDir, Took: res.Summary(), Seconds: fmt.Sprintf("%.1f", res.Total().Seconds()), Ports: res.Ports}))
	}
	if err := retire(a, input.SessionID, input.TranscriptPath); err != nil {
		return err
	}
	res.note(appendLog(logEntry{Time: time.Now(), Kind: "push", Session: input.SessionID, From: res.From, To: a.Host, FromDir: res.FromDir, ToDir: res.ToDir}))
	return json.NewEncoder(out).Encode(handedOff(a, res))
}

func handedOff(a armed, res Result) map[string]string {
	where := fmt.Sprintf("It's running %s, and with Remote Control it's in every Claude app on your account", res.Where)
	if len(res.ShownOn) > 0 {
		where = fmt.Sprintf("It's open in %s, and with Remote Control it's in every Claude app on your account", strings.Join(res.ShownOn, ", "))
	}
	if res.NewFolder != "" {
		where += fmt.Sprintf(". %s didn't have this project yet, so it now lives at %s", a.Host, res.NewFolder)
	}
	if forwarded := res.Forwarded(); forwarded != "" {
		where += ". " + forwarded
	}
	if len(res.Notes) > 0 {
		where += ". Notes: " + res.Noted()
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

func handoff(a armed, input stopInput) (Result, error) {
	host, err := loadHost(a.Host)
	if err != nil {
		return Result{}, err
	}
	return Handoff{
		HostName:       a.Host,
		Note:           a.Note,
		Stay:           a.Stay,
		Host:           host,
		SessionID:      input.SessionID,
		TranscriptPath: input.TranscriptPath,
		Cwd:            input.Cwd,
	}.Run()
}

func retire(a armed, sessionID, transcriptPath string) error {
	desktop := "0"
	if a.Desktop {
		desktop = "1"
	}
	cmd := exec.Command("sh", "-c", retireScript, "cielago", strconv.Itoa(a.PID), desktop, transcriptPath, a.Host, os.Getenv("REX_SESSION"), os.Getenv("REX_BLOCK"), sessionID)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	return cmd.Start()
}

func runSetupHere(dir, setup string, out io.Writer) error {
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/sh"
	}
	interrupted := make(chan os.Signal, 1)
	signal.Notify(interrupted, os.Interrupt)
	defer signal.Stop(interrupted)
	cmd := exec.Command(shell, "-lc", setup)
	cmd.Dir, cmd.Stdin, cmd.Stdout, cmd.Stderr = dir, os.Stdin, out, out
	return cmd.Run()
}

func setUpPulledProject(p *Project, root string, out io.Writer) string {
	if p == nil {
		return ""
	}
	var b strings.Builder
	if p.Setup != "" {
		fmt.Fprintf(out, "Running the project's setup: %s\n", p.Setup)
		if err := runSetupHere(root, p.Setup, out); err != nil {
			fmt.Fprintf(out, "Note: the setup failed (%v); the session continues anyway.\n", err)
			fmt.Fprintf(&b, "The project's %s setup (%s) failed here before you continued (%v). ", projectFile, p.Setup, err)
		} else {
			fmt.Fprintf(&b, "The project's %s setup (%s) ran here before you continued. ", projectFile, p.Setup)
		}
	}
	if len(p.Services) > 0 {
		fmt.Fprintf(&b, "Its services (%s) aren't running here; start them if you need them. ", strings.Join(p.Services, "; "))
	}
	return b.String()
}

func pullCommand(hostName, sessionID string) error {
	host, err := loadHost(hostName)
	if err != nil {
		return err
	}
	res, err := Pull{HostName: hostName, Host: host, SessionID: sessionID, Out: os.Stdout}.Run()
	if err != nil {
		return err
	}
	local, err := newLocal(context.Background())
	if err != nil {
		return err
	}
	res.note(appendLog(logEntry{Time: time.Now(), Kind: "pull", Session: res.Session, From: hostName, To: local.Name(), FromDir: res.FromDir, ToDir: res.ToDir}))
	res.note(closeForwards(res.Session))
	fmt.Printf("Pulled the session back from %s in %s.\n", hostName, res.Summary())
	if res.NewFolder != "" {
		fmt.Printf("This machine didn't have the project yet, so it now lives at %s.\n", res.NewFolder)
	}
	for _, note := range res.Notes {
		fmt.Printf("Note: %v\n", note)
	}
	res.PromptInfo.Project = setUpPulledProject(res.Project, res.ProjectRoot, os.Stdout)
	return resumeHere(res.ToDir, res.Session, res.Name, handoffPrompt(res.PromptInfo), res.Codex)
}
