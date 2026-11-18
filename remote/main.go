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
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"
)

var version = "dev"

const usage = `magi moves a Claude Code or Codex session to another machine, or sends Claude work there.

Usage:
  magi init                          set up this machine and a machine to hand off to
  magi doctor [host...]              check that everything a handoff needs is in place
  magi pull <host> [session-id] [--tab] [--json]
                                        bring a Claude Code or Codex session back from <host>
                                        and continue it here, or in a new tab with --tab
  magi move <session-id> <host> [--json] [--stay]
                                        move a Claude Code or Codex session on this machine to
                                        <host>, from outside it: now if it's idle, or when its
                                        turn ends; Codex needs codex = "<path>" for <host>
  magi hosts                         the configured hosts, one per line
  magi status                        recent handoffs, parked checkouts, and copies a failed
                                        handoff left running, which it stops
  magi unpark [dir]                  stop treating a checkout as parked
  magi allow [dir]                   let the project's .magi.toml run after handoffs
  magi open <folder> [--codex] [--model <model>] [--json]
                                        start a Claude (or Codex) session there, in a Rex session
                                        named after its folder; --resume <id> continues a session
                                        as a copy; --chat starts one in ~/Chats instead;
                                        --list prints the project folders it knows
  magi forwards                      ports this machine forwards to sessions that moved
  magi forwards close <session-id>   stop forwarding a moved session's ports
  magi tasks                         work sent to other machines, and its state
  magi tasks wait <id>               wait until a task finishes, then print how it ended
  magi tasks show <id>               a task's output; collects it if it's finished
  magi tasks cancel <id>             stop a task and remove its copy
  magi tasks clean                   collect finished tasks, drop day-idle copies and week-old tasks
  magi version
  magi help

Used by the Claude Code plugin:
  magi arm <host> <session-id> [-]   hand this session off when its turn ends;
                                        with -, read a note on what was running from stdin
  magi hook stop                     the plugin's Stop hook (reads the hook JSON on stdin)
  magi hook prompt                   the plugin's prompt hook, which refuses input in a session that moved
  magi hook start                    the plugin's SessionStart hook, which tells a session handed back what changed
  magi mcp                           the plugin's tool server, on stdin and stdout
  magi forward-run <session-id>      keeps a moved session's ports forwarded (started by a move)

For a live view of every session and machine in a Rex pane, install magi-overview.
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
	case len(args) == 2 && args[0] == "hook" && args[1] == "prompt":
		err = promptHook(os.Stdin, os.Stdout)
	case len(args) == 2 && args[0] == "hook" && args[1] == "start":
		err = startHook(os.Stdin, os.Stdout)
	case len(args) == 1 && args[0] == "init":
		err = initConfig(os.Stdin, os.Stdout)
	case len(args) >= 1 && args[0] == "doctor":
		err = doctor(args[1:], os.Stdout)
	case len(args) >= 2 && args[0] == "pull":
		err = pullCommand(args[1:])
	case len(args) == 1 && args[0] == "hosts":
		err = printHosts(os.Stdout)
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
	case len(args) == 1 && (args[0] == "help" || args[0] == "--help" || args[0] == "-h"):
		fmt.Print(usage)
	case len(args) == 1 && (args[0] == "version" || args[0] == "--version"):
		fmt.Println("magi", currentVersion())
	default:
		fmt.Fprint(os.Stderr, usage)
		os.Exit(2)
	}
	if errors.Is(err, errReported) {
		os.Exit(1)
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

func armedPath(sessionID, suffix string) (string, error) {
	if !sessionIDPattern.MatchString(sessionID) {
		return "", fmt.Errorf("%q is not a Claude session id", sessionID)
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".magi", "armed", sessionID+suffix), nil
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
		fetch, others := fetchedBy(hostName, sessionID)
		if fetch != nil {
			return fetch(out)
		}
		if len(others) > 0 {
			return fmt.Errorf("%w; magi's overview also sees %s", err, strings.Join(others, ", "))
		}
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

const (
	fetchTries      = 3
	fetchRetry      = 2 * time.Second
	fetchAnswerWait = 10 * time.Second
	fetchErrorGrace = 500 * time.Millisecond
)

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

func fetchedBy(hostName, sessionID string) (func(io.Writer) error, []string) {
	var others []string
	for attempt := range fetchTries {
		if attempt > 0 {
			time.Sleep(fetchRetry)
		}
		ctx, cancel := context.WithTimeout(context.Background(), fleetWait)
		fleet, err := readFleet(ctx)
		cancel()
		if err != nil {
			continue
		}
		others = others[:0]
		for _, m := range fleet.Machines {
			if m.Live && !m.Here {
				others = append(others, m.Name)
			}
		}
		if matchMachine(others, hostName) != "" {
			break
		}
	}
	target := matchMachine(others, hostName)
	if target == "" {
		return nil, others
	}
	return func(out io.Writer) error {
		program, err := exec.LookPath("magi-overview")
		if err != nil {
			return err
		}
		tasks, err := tasksDir()
		if err != nil {
			return err
		}
		dir := filepath.Dir(tasks)
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return err
		}
		logPath := filepath.Join(dir, "fetch-"+shortID(sessionID)+".log")
		log, err := os.Create(logPath)
		if err != nil {
			return err
		}
		defer log.Close()
		cmd := exec.Command(program, "move", sessionID, target)
		cmd.Stdout, cmd.Stderr = log, log
		cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
		if err := cmd.Start(); err != nil {
			return err
		}
		if err := awaitFetchAnswer(cmd, logPath); err != nil {
			return fmt.Errorf("%s didn't take this session back: %w", target, err)
		}
		fmt.Fprintf(out, "Asked %s to take this session back: it moves there as soon as this turn ends. End the turn now.\n", target)
		return nil
	}, nil
}

func awaitFetchAnswer(cmd *exec.Cmd, logPath string) error {
	exited := make(chan error, 1)
	go func() { exited <- cmd.Wait() }()
	answeredAt := time.Time{}
	for deadline := time.Now().Add(fetchAnswerWait); time.Now().Before(deadline); {
		select {
		case err := <-exited:
			if err == nil {
				return nil
			}
			text, _ := os.ReadFile(logPath)
			return errors.New(strings.TrimSpace(string(text)))
		case <-time.After(100 * time.Millisecond):
		}
		if answeredAt.IsZero() {
			if info, err := os.Stat(logPath); err == nil && info.Size() > 0 {
				answeredAt = time.Now()
			}
		} else if time.Since(answeredAt) > fetchErrorGrace {
			return nil
		}
	}
	return nil
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
	if os.Getenv("CLAUDE_PID") != strconv.Itoa(a.PID) {
		if a.Notify {
			return writeMoveResult(input.SessionID, moveReport{Event: "failed", Message: "the session that ended its turn wasn't the one armed to move"})
		}
		return nil
	}
	unlock, err := lockMove(input.SessionID)
	if err == nil {
		defer unlock()
		err = cleanStrayFor(input.SessionID)
	}
	if err != nil {
		return json.NewEncoder(out).Encode(map[string]string{"decision": "block", "reason": err.Error() + ", so this handoff didn't start. Tell the user."})
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT, syscall.SIGHUP)
	defer stop()

	res, err := handoff(ctx, a, input)
	if res.Stray != nil {
		res.note(saveStray(input.SessionID, *res.Stray))
	}
	if err != nil {
		reason := fmt.Sprintf("magi could not hand this session off to %s, so it is still running here: %v.", a.Host, err)
		if a.Notify {
			if writeErr := writeMoveResult(input.SessionID, moveReport{Event: "failed", Message: fmt.Sprintf("the handoff to %s failed, so the session keeps running here: %v", a.Host, err)}); writeErr != nil {
				reason += fmt.Sprintf(" Telling magi move about it also failed: %v.", writeErr)
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
	res.note(appendLog(logEntry{Time: time.Now(), Kind: "push", Session: input.SessionID, From: res.From, To: a.Host, FromDir: res.FromDir, ToDir: res.ToDir, Desktop: a.Desktop}))
	if a.Desktop && res.RemoteControl != "" {
		res.note(exec.Command("open", "claude://claude.ai/code/"+res.RemoteControl).Run())
	}
	return json.NewEncoder(out).Encode(handedOff(a, res))
}

func hookSession(in io.Reader) (home, sessionID string, err error) {
	var input stopInput
	if err := json.NewDecoder(in).Decode(&input); err != nil {
		return "", "", err
	}
	if !sessionIDPattern.MatchString(input.SessionID) {
		return "", "", nil
	}
	home, err = os.UserHomeDir()
	return home, input.SessionID, err
}

func promptHook(in io.Reader, out io.Writer) error {
	home, sessionID, err := hookSession(in)
	if err != nil || sessionID == "" {
		return err
	}
	host, err := os.ReadFile(filepath.Join(home, ".magi", "moved", sessionID))
	if errors.Is(err, os.ErrNotExist) {
		return passReturnNote(home, sessionID, "UserPromptSubmit", out)
	}
	if err != nil {
		return err
	}
	to := strings.TrimSpace(string(host))
	return json.NewEncoder(out).Encode(map[string]string{
		"decision": "block",
		"reason":   fmt.Sprintf("This session moved to %s, so this copy takes no more messages. Continue it there, or bring it back here with magi pull %s.", to, to),
	})
}

func startHook(in io.Reader, out io.Writer) error {
	home, sessionID, err := hookSession(in)
	if err != nil || sessionID == "" {
		return err
	}
	return passReturnNote(home, sessionID, "SessionStart", out)
}

func returnNotePath(home, sessionID string) string {
	return filepath.Join(home, ".magi", "returned", sessionID)
}

func passReturnNote(home, sessionID, event string, out io.Writer) error {
	path := returnNotePath(home, sessionID)
	note, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if err := os.Remove(path); err != nil {
		return err
	}
	return json.NewEncoder(out).Encode(map[string]any{
		"hookSpecificOutput": map[string]string{"hookEventName": event, "additionalContext": string(note)},
	})
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
	return map[string]string{
		"systemMessage": fmt.Sprintf("Handed off to %s in %s. %s. This copy is closing.", a.Host, res.Summary(), where),
	}
}

func handoff(ctx context.Context, a armed, input stopInput) (Result, error) {
	host, err := loadHost(a.Host)
	if err != nil {
		return Result{}, err
	}
	local, err := newLocal(ctx)
	if err != nil {
		return Result{}, err
	}
	return Handoff{
		HostName:       a.Host,
		Note:           stillRunning(a.Note, input.BackgroundTasks),
		Stay:           a.Stay || a.Desktop,
		Host:           host,
		SessionID:      input.SessionID,
		TranscriptPath: input.TranscriptPath,
		Cwd:            input.Cwd,
		Retire: func() error {
			if err := markMoved(local, input.SessionID, a.Host); err != nil {
				return err
			}
			return retire(a, input.SessionID, input.TranscriptPath)
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

func retire(a armed, sessionID, transcriptPath string) error {
	cmd := exec.Command("sh", "-c", retireScript, "magi", strconv.Itoa(a.PID), transcriptPath, a.Host, os.Getenv("REX_SESSION"), os.Getenv("REX_BLOCK"), sessionID, os.Getenv("TMUX_PANE"))
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

func pullCommand(args []string) error {
	var asJSON, inTab bool
	var rest []string
	for _, arg := range args {
		switch arg {
		case "--json":
			asJSON = true
		case "--tab":
			inTab = true
		default:
			rest = append(rest, arg)
		}
	}
	if len(rest) < 1 || len(rest) > 2 {
		return errors.New("usage: magi pull <host> [session-id] [--tab] [--json]")
	}
	hostName, sessionID := rest[0], ""
	if len(rest) == 2 {
		sessionID = rest[1]
	}
	out := io.Writer(os.Stdout)
	report := func(moveReport) {}
	if asJSON {
		out = os.Stderr
		encoder := json.NewEncoder(os.Stdout)
		report = func(r moveReport) {
			r.Session, r.Host = sessionID, hostName
			encoder.Encode(r)
		}
		report(moveReport{Event: "moving", OK: true, Message: fmt.Sprintf("Bringing session %s back from %s...", shortID(sessionID), hostName)})
	}
	pulled, err := pullHere(hostName, sessionID, out, inTab)
	if err != nil {
		report(moveReport{Event: "failed", Message: err.Error()})
		return err
	}
	report(moveReport{Event: "moved", OK: true, Message: pulled.message, ToDir: pulled.res.ToDir, Took: pulled.res.Summary(), Seconds: fmt.Sprintf("%.1f", pulled.res.Total().Seconds())})
	if inTab {
		return nil
	}
	if err := resumeHere(pulled.res.ToDir, pulled.res.Session, pulled.res.Name, pulled.prompt, pulled.res.Codex); err != nil {
		return fmt.Errorf("the session is back in %s, but starting it failed (%w); continue it with %s", pulled.res.ToDir, err, resumeHint(remoteSession{ID: pulled.res.Session, Codex: pulled.res.Codex}))
	}
	return nil
}

type pulledHere struct {
	res     Pulled
	prompt  string
	message string
}

func pullHere(hostName, sessionID string, out io.Writer, inTab bool) (pulledHere, error) {
	host, err := loadHost(hostName)
	if err != nil {
		return pulledHere{}, err
	}
	if sessionIDPattern.MatchString(sessionID) {
		unlock, err := lockMove(sessionID)
		if err != nil {
			return pulledHere{}, err
		}
		defer unlock()
	}
	res, err := Pull{HostName: hostName, Host: host, SessionID: sessionID, Out: out}.Run()
	if err != nil {
		return pulledHere{}, err
	}
	local, err := newLocal(context.Background())
	if err != nil {
		return pulledHere{}, err
	}
	res.note(appendLog(logEntry{Time: time.Now(), Kind: "pull", Session: res.Session, From: hostName, To: local.Name(), FromDir: res.FromDir, ToDir: res.ToDir}))
	res.note(closeForwards(res.Session))
	fmt.Fprintf(out, "Pulled the session back from %s in %s.\n", hostName, res.Summary())
	if res.NewFolder != "" {
		fmt.Fprintf(out, "This machine didn't have the project yet, so it now lives at %s.\n", res.NewFolder)
	}
	for _, note := range res.Notes {
		fmt.Fprintf(out, "Note: %v\n", note)
	}
	res.PromptInfo.Project = setUpPulledProject(res.Project, res.ProjectRoot, out)
	prompt := handoffPrompt(res.PromptInfo)
	if inTab {
		before := len(res.Notes)
		if err := openPulledTab(local, &res, prompt); err != nil {
			return pulledHere{}, fmt.Errorf("the session is back in %s, but its tab didn't open (%w); continue it there with %s", res.ToDir, err, resumeHint(remoteSession{ID: res.Session, Codex: res.Codex}))
		}
		for _, note := range res.Notes[before:] {
			fmt.Fprintf(out, "Note: %v\n", note)
		}
	}
	message := fmt.Sprintf("Brought session %s back from %s to %s", shortID(res.Session), hostName, res.ToDir)
	if res.InDesktop {
		message += ", open in Claude Desktop"
	}
	if len(res.Notes) > 0 {
		message += " (" + res.Noted() + ")"
	}
	return pulledHere{res: res, prompt: prompt, message: message}, nil
}

func openPulledTab(local *Local, res *Pulled, prompt string) error {
	if !res.Codex && leftFromDesktop(res.Session) {
		info := res.PromptInfo
		info.WithMessage = true
		err := openInDesktop(res.ToDir, res.Session, handoffPrompt(info))
		if err == nil {
			res.InDesktop = true
			return nil
		}
		res.note(fmt.Errorf("it came from Claude Desktop, but opening it there failed (%w), so it opens in a tab", err))
	}
	roots, err := openRoots(local)
	if err != nil {
		return err
	}
	root := res.ProjectRoot
	if root == "" {
		root = res.ToDir
	}
	command, label := resumeCommand(res.Session, res.Name, prompt, res.Codex), claudeTabLabel(res.Name)
	if res.Codex {
		label = codexTabLabel(res.Name)
	}
	program, err := exec.LookPath(command[0])
	if err != nil {
		return fmt.Errorf("%s isn't installed on %s", command[0], local.Name())
	}
	var launcher Launcher = Tmux{remote: local}
	if _, err := exec.LookPath("rex"); err == nil {
		launcher = Rex{remote: local}
	}
	tab, err := launcher.Start(localGroup(root, roots), label, res.ToDir, append([]string{program}, command[1:]...), true)
	if err != nil {
		return err
	}
	if _, err := launcher.Show(tab); err != nil {
		res.note(fmt.Errorf("showing the new tab failed: %w", err))
	}
	return nil
}

func printHosts(out io.Writer) error {
	cfg, _, err := loadConfig()
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	for _, name := range cfg.Order {
		if _, err := fmt.Fprintln(out, name); err != nil {
			return err
		}
	}
	return nil
}
