package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/felitrejos/magi/remote/internal/store"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const serverInstructions = `magi sends work from this session to the user's other machines over SSH: a shell command (run_elsewhere), a self-contained task for a background Claude (delegate), or a new interactive session (session_elsewhere). Each runs in its own copy of this project's working tree, in a tmux window the user can watch, and never touches the user's checkouts there. When you collect a delegate with task_result, its changes are applied to this working tree, unless you passed keep_branch. magi also moves sessions between machines (move_session, or the handoff skill for this session), sets up another machine or checks one (setup_machine), and says what it has going on here (status).

Starting work on another machine is the user's call. Before run_elsewhere, delegate or session_elsewhere: call machines, propose one machine (the one machines suggests, the first in the user's list with room, unless the user named one), say exactly what will run there, and let the user choose: run it there, run it here instead, or don't run it. Wait for the user's clear answer. Ask even when the user tells you to pick the machine or to run it wherever fits best: choosing the machine is your part, agreeing to it is theirs. Then start it and carry on with other work. Moving a session with move_session and setting up a machine with setup_machine, which installs the magi plugin there and a background watcher here, are the user's call too: say exactly what will happen, and wait for the user's clear yes. allow_project asks the user itself.

To be told when a run_elsewhere or delegate task is done, run magi tasks wait <task id> as a background shell command: it exits when the task finishes, and then you collect it with task_result. When a delegated Claude should keep going until something passes, such as running CI and fixing what fails, say so in its task. If task_result shows it isn't fully done, send what's left with task_reply and wait again. If it says the changes couldn't be applied, merge them in yourself from its branch, then tell the user what changed. To be told when a session isn't busy any more, such as one you'd move, run magi wait <session id> as a background shell command. Don't poll in a loop.`

const delegateSystemPrompt = `You are a delegated task started by magi from another machine. You work in a throwaway copy of the project on this machine; nobody is watching to answer questions or approve permission prompts. Tools that would need approval are denied: work around them, and say what you couldn't do. Work only inside this folder. Don't commit, push, open pull requests or change git configuration: leave your changes in the working tree, and magi brings them back to the caller. Finish with a short report: what you did, which files you changed, how you checked it, and anything left undone.`

var modelPattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:\[\]-]*$`)

var delegateModes = []string{"acceptEdits", "dontAsk", "auto"}

const delegateVar = "MAGI_DELEGATE"

type machinesInput struct {
	Needs needsInput `json:"needs,omitempty" jsonschema:"what the work has to run on, when it matters, for example a Linux x86_64 build"`
}

type runInput struct {
	Command string `json:"command" jsonschema:"the shell command to run from the project's folder, for example go test ./..."`
	Host    string `json:"host,omitempty" jsonschema:"the magi host the user agreed to, as listed by machines; left out, magi takes the first machine in the user's list that has room at this moment, which may not be the one you proposed"`
	Dir     string `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	Setup   *bool  `json:"setup,omitempty" jsonschema:"run the project's approved .magi.toml setup first, such as installing dependencies (default true)"`
}

type delegateInput struct {
	Task           string   `json:"task" jsonschema:"a self-contained brief, since the remote Claude doesn't see this conversation and can't ask questions: the goal, the relevant files and what you already know about them, constraints, how to check the work, and what done looks like"`
	Host           string   `json:"host,omitempty" jsonschema:"the magi host the user agreed to, as listed by machines; left out, magi takes the first machine in the user's list that has room at this moment, which may not be the one you proposed"`
	Dir            string   `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	AllowedTools   []string `json:"allowed_tools,omitempty" jsonschema:"extra permission rules the remote Claude may use without asking, for example Bash(go test:*) or Bash(npm run lint); only what the user agreed to. When the task revolves around one tool, grant the whole tool, such as Bash(docker:*), rather than a few of its subcommands, or the remote Claude gets stuck on the first one you didn't list"`
	PermissionMode string   `json:"permission_mode,omitempty" jsonschema:"acceptEdits (default: read and edit files in its copy), dontAsk (only what allowed_tools and that machine's settings allow) or auto (Claude Code's auto mode, which reviews each action)"`
	Model          string   `json:"model,omitempty" jsonschema:"model for the remote Claude, such as sonnet or opus; leave out for that machine's default"`
	Setup          *bool    `json:"setup,omitempty" jsonschema:"run the project's approved .magi.toml setup first (default true)"`
	KeepBranch     bool     `json:"keep_branch,omitempty" jsonschema:"leave its changes only on the branch magi/task-<id> instead of applying them to this working tree; when the user wants to review them first"`
}

type sessionInput struct {
	Host   string `json:"host,omitempty" jsonschema:"the magi host the user agreed to, as listed by machines; left out, magi takes the first machine in the user's list that has room at this moment, which may not be the one you proposed"`
	Dir    string `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	Prompt string `json:"prompt,omitempty" jsonschema:"an optional first message for the new session"`
}

type taskInput struct {
	TaskID string `json:"task_id" jsonschema:"the task id a start tool returned"`
}

type replyInput struct {
	TaskID  string `json:"task_id" jsonschema:"the id of a delegate task"`
	Message string `json:"message" jsonschema:"the follow-up for the remote Claude, which remembers the task and what it did so far"`
}

type sessionQuery struct {
	Session string `json:"session" jsonschema:"the session: the start of its id, or at least four letters of its title or folder"`
}

type moveInput struct {
	Session   string `json:"session" jsonschema:"the session to move: the start of its id, or at least four letters of its title or folder"`
	To        string `json:"to" jsonschema:"where the user agreed it goes: a host as machines lists it, or this machine's name as sessions lists it first, to bring a session here"`
	Confirmed bool   `json:"confirmed,omitempty" jsonschema:"true only after the user said yes to the list of what goes that a call without it returned"`
}

type allowInput struct {
	Dir    string `json:"dir" jsonschema:"absolute path of the project folder, as the note from magi gave it"`
	SHA256 string `json:"sha256" jsonschema:"the sha256 of the .magi.toml the user agreed to, exactly as the note from magi gave it"`
}

type dirInput struct {
	Dir string `json:"dir" jsonschema:"absolute path of the checkout, as status lists it"`
}

type optionalTaskInput struct {
	TaskID string `json:"task_id,omitempty" jsonschema:"the task id; leave out to list this machine's tasks"`
}

func addTool[In any](server *mcp.Server, tool *mcp.Tool, run func(context.Context, In) (string, error)) {
	mcp.AddTool(server, tool, func(ctx context.Context, _ *mcp.CallToolRequest, in In) (*mcp.CallToolResult, any, error) {
		text, err := run(ctx, in)
		if err != nil {
			return nil, nil, err
		}
		return said(text), nil, nil
	})
}

func projectDir(dir string) (string, error) {
	if dir != "" {
		if !filepath.IsAbs(dir) {
			return "", fmt.Errorf("dir must be an absolute path, got %q", dir)
		}
		return dir, nil
	}
	if dir := os.Getenv("CLAUDE_PROJECT_DIR"); dir != "" {
		return dir, nil
	}
	return os.Getwd()
}

func setupWanted(setup *bool) bool {
	return setup == nil || *setup
}

func delegateArgs(in delegateInput) ([]string, error) {
	mode := in.PermissionMode
	if mode == "" {
		mode = "acceptEdits"
	}
	if !slices.Contains(delegateModes, mode) {
		return nil, fmt.Errorf("permission_mode %q isn't allowed; use one of %s", mode, strings.Join(delegateModes, ", "))
	}
	var allowed []string
	for _, tool := range in.AllowedTools {
		if tool = strings.TrimSpace(tool); tool != "" {
			if strings.HasPrefix(tool, "-") || strings.ContainsFunc(tool, func(r rune) bool { return r < 0x20 || r == 0x7f }) {
				return nil, fmt.Errorf("allowed_tools: %q isn't a permission rule", tool)
			}
			allowed = append(allowed, tool)
		}
	}
	prompt := delegateSystemPrompt
	if len(allowed) > 0 {
		prompt += " You may run these without approval: " + strings.Join(allowed, ", ") + ". Run each as its own simple command, not chained with && or ; to anything else, since a chained command needs every part allowed. A denied command doesn't block these: keep going with them."
	}
	args := []string{"-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode, "--permission-prompts", "none", "--append-system-prompt", prompt}
	if in.Model != "" {
		if !modelPattern.MatchString(in.Model) {
			return nil, fmt.Errorf("model %q isn't a model name", in.Model)
		}
		args = append(args, "--model", in.Model)
	}
	if len(allowed) > 0 {
		args = append(append(args, "--allowedTools"), allowed...)
	}
	return args, nil
}

func startElsewhere(ctx context.Context, host string, spec TaskSpec) (string, error) {
	spec.Caller = callerPID
	picks, skipped, err := chooseHosts(ctx, host, spec.Kind != kindRun)
	if err != nil {
		return "", err
	}
	for _, c := range picks {
		spec.Host = c.Host
		s, err := startTask(ctx, spec)
		var full *noRoomError
		if errors.As(err, &full) {
			skipped = append(skipped, full.Error())
			continue
		}
		if err != nil {
			return "", fmt.Errorf("starting it on %s failed: %w", c.Host, err)
		}
		why := c.Why
		if len(skipped) > 0 {
			why += "; skipped " + strings.Join(skipped, "; ")
		}
		return describeStarted(s, why), nil
	}
	return "", fmt.Errorf("no machine has room for it: %s", strings.Join(skipped, "; "))
}

func newMCPServer() *mcp.Server {
	server := mcp.NewServer(&mcp.Implementation{Name: "magi", Title: "magi", Version: currentVersion()}, &mcp.ServerOptions{Instructions: serverInstructions})
	readOnly := &mcp.ToolAnnotations{ReadOnlyHint: true}
	no := false

	addTool(server, &mcp.Tool{
		Name:        "machines",
		Annotations: readOnly,
		Description: "List the machines magi can send work to, each with its operating system, architecture and the tools it has (build toolchains, and Docker or Podman when they're running), its load read over SSH (cores, load average, memory and Claude sessions working there), whether it answers over SSH, magi tasks running on it, each host's budget and whether it has room, the machine magi suggests, and this machine's own load for comparison. When the work needs a platform or a tool, such as a Linux build from a Mac, pass needs: machines then ranks the places that can do it, best first: one that runs it natively, then a container on whichever machine has room, this machine included. Prefer a machine that runs it natively over a container or emulation here. Read-only. Call it before proposing where to run something, and tell the user which machine you'd pick and why.",
	}, func(ctx context.Context, in machinesInput) (string, error) {
		cfg, path, err := loadConfig()
		if err != nil {
			return "", err
		}
		if len(cfg.Hosts) == 0 {
			return "", fmt.Errorf("no hosts in %s yet; setup_machine adds one", path)
		}
		return describeMachines(surveyFleet(ctx, cfg, true), in.Needs), nil
	})

	addTool(server, &mcp.Tool{
		Name:        "sessions",
		Annotations: readOnly,
		Description: "List the Claude Code sessions running on this machine and on every machine in the user's magi config, read over SSH. The first line says which sessions are waiting on the user, so this one call answers \"does anything need me?\". Then each session's id, title, state (busy, waiting and on what, or idle), folder, and its Remote Control link when it has one. Read-only.",
	}, func(ctx context.Context, _ struct{}) (string, error) {
		all, err := everySession(ctx)
		if err != nil {
			return "", err
		}
		return describeSessions(all), nil
	})

	addTool(server, &mcp.Tool{
		Name:        "diff",
		Annotations: readOnly,
		Description: "Show a running Claude Code session's uncommitted changes (git diff against HEAD, plus new untracked files), on whichever machine it runs, read over SSH. Long diffs keep their end. Read-only.",
	}, func(ctx context.Context, in sessionQuery) (string, error) {
		all, err := everySession(ctx)
		if err != nil {
			return "", err
		}
		m, s, err := findAnySession(all, in.Session)
		if err != nil {
			return "", err
		}
		return sessionDiff(m.Machine, s)
	})

	addTool(server, &mcp.Tool{
		Name:        "move_session",
		Description: "Move a Claude Code session, other than this one, between this machine and another, with its conversation and its working tree. A session that runs here goes to a host as machines lists it; a session that runs on a host comes here when to is this machine's name, and opens here in a tmux window, with Remote Control. A session that was quit, here or on a host, moves the same way when you pass its full session id, as status lists it. Returns what went across and where it now runs. When the copy includes secret files, files git doesn't track, or 50 MB or more, a call without confirmed moves nothing and returns what would go: show it to the user, and after their yes call again with confirmed set to true. A session that's in the middle of a turn or waiting on the user isn't moved: run magi wait <session id> as a background shell command, then move it if that says it's idle, or ask the user to answer it first if that says it's waiting on them. To move this session itself, use the handoff skill. Moving a session is the user's call: before calling, say which session goes to which machine, and wait for the user's clear yes.",
	}, moveSession)

	addTool(server, &mcp.Tool{
		Name:        "setup_machine",
		Description: "Set up magi with another machine, or check one that's set up already, for \"set up magi with my homelab\" or \"check my homelab\". It checks key-based SSH to it, finds Claude Code there, saves the machine in magi's config, installs the magi plugin there if it's missing, starts magi watch here in the background so sessions can be handed back to this machine, checks both machines, then moves a throwaway session there and back, which takes three short Claude turns on the user's account and a minute or two. Returns each check, and what only the user can do, such as logging in to Claude there. Installing things on another machine is the user's call: before calling, say that it installs the magi plugin on that machine and a background watcher here, and wait for the user's clear yes.",
	}, func(_ context.Context, in setupInput) (string, error) {
		return setupMachine(in)
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "allow_project",
		Description: "Let a project's .magi.toml run on this machine after moves and before run_elsewhere and delegate tasks: its setup command, its services and its port forwards. magi shows the user the file's exact commands in a dialog and allows them only if the user accepts there, and only the file whose sha256 you pass, so if it changes it needs allowing again. Pass dir and sha256 exactly as a note from magi gave them; with any other sha256 it allows nothing and returns the file's commands and sha256. Call it when the user wants the project's setup to run.",
	}, func(_ context.Context, req *mcp.CallToolRequest, in allowInput) (*mcp.CallToolResult, any, error) {
		res, err := allowProject(in.Dir, in.SHA256, req.Params.InputResponses)
		return res, nil, err
	})

	addTool(server, &mcp.Tool{
		Name:        "status",
		Description: "What magi has going on on this machine: moves that were cut off, sessions waiting for another machine to take them back, recent handoffs, parked checkouts and ports forwarded to sessions that moved away. magi settles a cut-off move on its own, and calling this settles any it can right away; what's left says what the user has to decide, such as which copy to keep when both were used. Call it when a move didn't finish or the user asks what magi did.",
	}, func(context.Context, struct{}) (string, error) {
		var b strings.Builder
		err := status(&b)
		return b.String(), err
	})

	addTool(server, &mcp.Tool{
		Name:        "unpark",
		Description: "Stop treating a checkout on this machine as parked. A session that moved away leaves its checkout parked, and when the session comes back, its changes replace the uncommitted ones there, unless they were edited meanwhile. After unpark, the uncommitted changes there count as the user's own, so a session coming back is refused until they're committed or stashed. Call it when the user wants to keep working in that checkout. status lists the parked ones.",
	}, func(_ context.Context, in dirInput) (string, error) {
		var b strings.Builder
		err := unpark(in.Dir, &b)
		return b.String(), err
	})

	addTool(server, &mcp.Tool{
		Name:        "run_elsewhere",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Run a shell command, such as a slow test suite or a build, on another machine, in a copy of this project's exact working tree (commits, staged and unstaged changes, untracked and secret files). It copies the working tree first, usually in seconds, then returns a task id; the command runs in the background in a tab the user can watch, so long runs never block or time out. The copy is separate from any checkout on that machine and is removed when the result is collected; this session and this checkout are untouched. Dependencies such as node_modules aren't copied, but the project's approved .magi.toml setup runs first. Before calling: call machines, tell the user which machine you propose (the one machines suggests unless they named one) and the exact command, and wait for their OK. Afterwards run magi tasks wait <task id> as a background shell command to be told when it finishes, then task_result for the exit code and output.",
	}, func(ctx context.Context, in runInput) (string, error) {
		if strings.TrimSpace(in.Command) == "" {
			return "", fmt.Errorf("command is empty")
		}
		dir, err := projectDir(in.Dir)
		if err != nil {
			return "", err
		}
		return startElsewhere(ctx, in.Host, TaskSpec{Kind: kindRun, Dir: dir, Command: in.Command, Setup: setupWanted(in.Setup)})
	})

	addTool(server, &mcp.Tool{
		Name:        "delegate",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Give a self-contained task to a background Claude on another machine: a subagent that runs elsewhere, in its own copy of this project's working tree. It copies the working tree first, usually in seconds, then returns a task id. When it's done, task_result returns its final message, and applies its file changes to this working tree, keeping them on the branch magi/task-<id> too; with keep_branch they're only put on that branch. It runs headless (claude -p) in acceptEdits mode by default: it reads and edits files in its copy, and anything else that would ask for permission, such as shell commands or network access, is denied unless allowed_tools or that machine's own settings allow it. It never runs with permission checks bypassed. It can't ask you questions, so put everything it needs in task. Before calling: call machines, then tell the user the machine you propose, what the task is, and any allowed_tools you want to grant, and wait for their OK.",
	}, func(ctx context.Context, in delegateInput) (string, error) {
		if strings.TrimSpace(in.Task) == "" {
			return "", fmt.Errorf("task is empty")
		}
		args, err := delegateArgs(in)
		if err != nil {
			return "", err
		}
		dir, err := projectDir(in.Dir)
		if err != nil {
			return "", err
		}
		return startElsewhere(ctx, in.Host, TaskSpec{Kind: kindDelegate, Dir: dir, Prompt: in.Task, Claude: args, Setup: setupWanted(in.Setup), Keep: in.KeepBranch})
	})

	addTool(server, &mcp.Tool{
		Name:        "session_elsewhere",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Open a new interactive Claude session for this project on another machine, in its own copy of this project's working tree, in a tmux window, with Remote Control so it's also in every Claude app on their account. For when the user wants to work on something in parallel on a lighter machine. This session keeps running. Only with the user's clear yes: first call machines, then say which machine and what the new session will start with, and wait. When the user quits that session, task_result brings its changes back as a branch.",
	}, func(ctx context.Context, in sessionInput) (string, error) {
		dir, err := projectDir(in.Dir)
		if err != nil {
			return "", err
		}
		return startElsewhere(ctx, in.Host, TaskSpec{Kind: kindSession, Dir: dir, Prompt: in.Prompt})
	})

	addTool(server, &mcp.Tool{
		Name:        "task_status",
		Annotations: readOnly,
		Description: "Check a task started with run_elsewhere, delegate or session_elsewhere: still running or finished, with its latest output. Without task_id, lists this machine's tasks. Read-only: it doesn't collect anything. Between checks, carry on with other work instead of polling in a loop.",
	}, func(_ context.Context, in optionalTaskInput) (string, error) {
		if in.TaskID == "" {
			var b strings.Builder
			if err := printTasks(&b); err != nil {
				return "", err
			}
			return b.String(), nil
		}
		r, err := taskStatus(in.TaskID)
		if err != nil {
			return "", err
		}
		return describeReport(r), nil
	})

	addTool(server, &mcp.Tool{
		Name:        "task_result",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no, IdempotentHint: true},
		Description: "Collect a finished task. A run_elsewhere task gives its exit code, the end of its output and the path of the full log on this machine. A delegate task gives Claude's final message and applies its changes to this working tree, keeping them on a branch too (only on the branch with keep_branch, or when they clash with changes made here since); a session_elsewhere task gives its changes as a branch. Collecting closes the task's tab and removes its copy on the other machine, except a finished delegate's copy: that stays for task_reply until cancel_task removes it or it has been idle for a day. If the task is still running, it says so and changes nothing. Calling it again returns the same result.",
	}, func(_ context.Context, in taskInput) (string, error) {
		r, err := collectTask(in.TaskID, false)
		if err != nil {
			return "", err
		}
		return describeReport(r) + sweepTasks(), nil
	})

	addTool(server, &mcp.Tool{
		Name:        "task_reply",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Send a follow-up to a delegated Claude once its last turn has finished, for example \"now also fix the lint errors\". It resumes the same Claude session in the same copy on the same machine, so it keeps everything it knew and did; the follow-up runs as a new turn of the same task, with the same permissions. Returns at once; run magi tasks wait <task id> as a background shell command to be told when the turn is done, then collect it with task_result, which returns its new message and applies that turn's changes the same way. It isn't sent when that machine's budget has no room. Tell the user what you're asking it before you send it.",
	}, func(ctx context.Context, in replyInput) (string, error) {
		s, err := replyTask(ctx, in.TaskID, in.Message)
		if err != nil {
			return "", err
		}
		return fmt.Sprintf("Sent the follow-up to task %s on %s as turn %d; it runs %s, tab %q. To be told when it's done, run `magi tasks wait %s` as a background shell command; then collect it with task_result.\n", s.Task.ID, s.Task.Host, s.Task.Turn, s.Task.Where, s.Task.Label, s.Task.ID), nil
	})

	addTool(server, &mcp.Tool{
		Name:        "cancel_task",
		Description: "Stop a running task, close its tab and remove its copy on the other machine; its unfinished changes are discarded. A run_elsewhere or delegate task's output so far is kept in a log on this machine; a session_elsewhere task keeps no output. Also use it to remove a finished delegate's copy once you're done sending it follow-ups. Ask the user before stopping a running task.",
	}, func(_ context.Context, in taskInput) (string, error) {
		r, err := collectTask(in.TaskID, true)
		if err != nil {
			return "", err
		}
		return describeReport(r), nil
	})
	addTool(server, &mcp.Tool{
		Name:        "uninstall",
		Description: "Stop magi on this machine: stop and remove the magi watch service and remove magi's downloaded binaries in ~/.magi/bin. magi's config and records stay. The magi plugin itself is removed by the user in Claude Code with /plugin. Removing it is the user's call: say what it removes, and call this only after the user's clear yes.",
	}, func(context.Context, struct{}) (string, error) {
		return uninstall()
	})
	if os.Getenv(delegateVar) != "" {
		server.RemoveTools("setup_machine", "move_session", "allow_project", "unpark", "run_elsewhere", "delegate", "session_elsewhere", "task_reply", "task_result", "cancel_task", "uninstall")
	}
	return server
}

var callerPID int

func serveMCP(ctx context.Context, transport mcp.Transport) error {
	callerPID = os.Getppid()
	go func() {
		sweepMoves()
		sweepTasks()
	}()
	return newMCPServer().Run(ctx, transport)
}

func moveSession(ctx context.Context, in moveInput) (string, error) {
	all, err := everySession(ctx)
	if err != nil {
		return "", err
	}
	m, s, err := findAnySession(all, in.Session)
	if err != nil && store.SessionIDPattern.MatchString(in.Session) {
		m, s, err = findQuit(all, in.Session, err)
	}
	if err != nil {
		for _, o := range all {
			if o.Err != nil {
				err = fmt.Errorf("%w; %s couldn't be read: %w", err, o.Name, o.Err)
			}
		}
		return "", err
	}
	here := all[0].Name
	switch {
	case m.Name == in.To:
		return "", fmt.Errorf("%s already runs there", sessionLabel(m.Name, s))
	case m.Name == here && s.PID == callerPID:
		return "", fmt.Errorf("%s is this session; to move it, use the handoff skill", sessionLabel(m.Name, s))
	case m.Name != here && in.To != here:
		return "", fmt.Errorf("%s runs on %s, and magi moves a session between this machine and another; move it here first, then on to %s", sessionLabel(m.Name, s), m.Name, in.To)
	case s.Status == "busy" || s.Status == "waiting" || m.Name != here && s.Status != "idle":
		return "", busyError(remoteSession{ID: s.SessionID})
	}
	if !in.Confirmed {
		if block, err := askFirst(m, here, in.To, s.SessionID); err != nil || block != "" {
			return block, err
		}
	}
	var out strings.Builder
	if m.Name == here {
		err = mover{session: s.SessionID, host: in.To, out: &out}.run()
	} else {
		var message string
		if message, err = pullHere(m.Name, s.SessionID, "", &out, true); err == nil {
			fmt.Fprintln(&out, message+".")
		}
	}
	if err != nil {
		return "", fmt.Errorf("%s%w", out.String(), err)
	}
	return out.String(), nil
}

func askFirst(m machineSessions, here, to, sessionID string) (string, error) {
	hostName, toName := to, to
	if m.Name != here {
		hostName, toName = m.Name, here
	}
	host, err := loadHost(hostName)
	if err != nil {
		return "", err
	}
	block, ask, err := describeSession(m.Machine, toName, host, sessionID, m.Name == here)
	if err != nil || !ask {
		return "", err
	}
	return block + "\nNothing moved: this copy needs the user's yes first. Show the user what goes, naming the untracked and secret files, and after their yes call move_session again with confirmed set to true.", nil
}

func findQuit(all []machineSessions, id string, notRunning error) (machineSessions, liveSession, error) {
	for _, m := range all {
		if m.Machine == nil {
			continue
		}
		if _, err := findSession(m.Machine, id); err == nil {
			return m, liveSession{SessionID: id, Status: "idle"}, nil
		}
	}
	return machineSessions{}, liveSession{}, notRunning
}
