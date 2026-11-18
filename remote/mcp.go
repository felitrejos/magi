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
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const serverInstructions = `magi sends work from this session to the user's other machines over SSH: a shell command (run_elsewhere), a self-contained task for a background Claude (delegate), or a new interactive session (session_elsewhere). Each runs in its own copy of this project's working tree, in a Rex or tmux tab the user can watch, and never touches the user's checkouts there. When you collect a delegate with task_result, its changes are applied to this working tree, unless you passed keep_branch.

Starting work on another machine is the user's call. Before run_elsewhere, delegate or session_elsewhere: call machines, propose one machine (the one machines suggests, the first in the user's list with room, unless the user named one), say exactly what will run there, and let the user choose: run it there, run it here instead, or don't run it. Wait for the user's clear answer. Ask even when the user tells you to pick the machine or to run it wherever fits best: choosing the machine is your part, agreeing to it is theirs. Then start it and carry on with other work.

To be told when a run_elsewhere or delegate task is done, run magi tasks wait <task id> as a background shell command: it exits when the task finishes, and then you collect it with task_result. When a delegated Claude should keep going until something passes, such as running CI and fixing what fails, say so in its task. If task_result shows it isn't fully done, send what's left with task_reply and wait again. If it says the changes couldn't be applied, merge them in yourself from its branch, then tell the user what changed. Don't poll in a loop.`

const delegateSystemPrompt = `You are a delegated task started by magi from another machine. You work in a throwaway copy of the project on this machine; nobody is watching to answer questions or approve permission prompts. Tools that would need approval are denied: work around them, and say what you couldn't do. Work only inside this folder. Don't commit, push, open pull requests or change git configuration: leave your changes in the working tree, and magi brings them back to the caller. Finish with a short report: what you did, which files you changed, how you checked it, and anything left undone.`

var modelPattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:\[\]-]*$`)

var delegateModes = []string{"acceptEdits", "dontAsk", "auto"}

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
	AllowedTools   []string `json:"allowed_tools,omitempty" jsonschema:"extra permission rules the remote Claude may use without asking, for example Bash(go test:*) or Bash(npm run lint); only what the user agreed to"`
	PermissionMode string   `json:"permission_mode,omitempty" jsonschema:"acceptEdits (default: read and edit files in its copy), dontAsk (only what allowed_tools and that machine's settings allow) or auto (Claude Code's auto mode, which reviews each action)"`
	Model          string   `json:"model,omitempty" jsonschema:"model for the remote Claude, such as sonnet or opus; leave out for that machine's default"`
	Setup          *bool    `json:"setup,omitempty" jsonschema:"run the project's approved .magi.toml setup first (default true)"`
	DespitePlan    bool     `json:"despite_plan,omitempty" jsonschema:"start it even though the Claude plan is nearly used up; only when the user said to go ahead anyway"`
	KeepBranch     bool     `json:"keep_branch,omitempty" jsonschema:"leave its changes only on the branch magi/task-<id> instead of applying them to this working tree; when the user wants to review them first"`
}

type sessionInput struct {
	Host        string `json:"host,omitempty" jsonschema:"the magi host the user agreed to, as listed by machines; left out, magi takes the first machine in the user's list that has room at this moment, which may not be the one you proposed"`
	Dir         string `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	Prompt      string `json:"prompt,omitempty" jsonschema:"an optional first message for the new session"`
	DespitePlan bool   `json:"despite_plan,omitempty" jsonschema:"open it even though the Claude plan is nearly used up; only when the user said to go ahead anyway"`
}

type taskInput struct {
	TaskID string `json:"task_id" jsonschema:"the task id a start tool returned"`
}

type replyInput struct {
	TaskID      string `json:"task_id" jsonschema:"the id of a delegate task"`
	Message     string `json:"message" jsonschema:"the follow-up for the remote Claude, which remembers the task and what it did so far"`
	DespitePlan bool   `json:"despite_plan,omitempty" jsonschema:"send it even though the Claude plan is nearly used up; only when the user said to go ahead anyway"`
}

type sessionQuery struct {
	Session string `json:"session" jsonschema:"the session: the start of its id, or at least four letters of its title or folder"`
}

type optionalTaskInput struct {
	TaskID string `json:"task_id,omitempty" jsonschema:"the task id; leave out to list this machine's tasks"`
}

func textResult(text string) *mcp.CallToolResult {
	return &mcp.CallToolResult{Content: []mcp.Content{&mcp.TextContent{Text: text}}}
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
	args := []string{"-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode, "--permission-prompts", "none", "--append-system-prompt", delegateSystemPrompt}
	if in.Model != "" {
		if !modelPattern.MatchString(in.Model) {
			return nil, fmt.Errorf("model %q isn't a model name", in.Model)
		}
		args = append(args, "--model", in.Model)
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
	if len(allowed) > 0 {
		args = append(append(args, "--allowedTools"), allowed...)
	}
	return args, nil
}

func planGate(plans []plan, despite bool) (string, error) {
	v := checkPlan(plans, "claude", time.Now())
	switch {
	case v.Refuse != "" && !despite:
		return "", fmt.Errorf("not started: %s", v.Refuse)
	case v.Refuse != "":
		return fmt.Sprintf("Started although %s, as the user asked.\n", strings.SplitN(v.Refuse, ";", 2)[0]), nil
	case v.Warn != "":
		return fmt.Sprintf("Heads-up: %s.\n", v.Warn), nil
	}
	return "", nil
}

func startElsewhere(ctx context.Context, host string, spec TaskSpec, despite bool) (string, error) {
	spec.Caller = self
	agents := spec.Kind != kindRun
	picks, skipped, plans, err := chooseHosts(ctx, host, agents)
	if err != nil {
		return "", err
	}
	var heads string
	if agents {
		if heads, err = planGate(plans, despite); err != nil {
			return "", err
		}
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
		return heads + describeStarted(s, why), nil
	}
	return "", fmt.Errorf("no machine has room for it: %s", strings.Join(skipped, "; "))
}

func newMCPServer() *mcp.Server {
	server := mcp.NewServer(&mcp.Implementation{Name: "magi", Title: "magi", Version: currentVersion()}, &mcp.ServerOptions{Instructions: serverInstructions + overviewInstructions})
	readOnly := &mcp.ToolAnnotations{ReadOnlyHint: true}
	no := false

	mcp.AddTool(server, &mcp.Tool{
		Name:        "machines",
		Annotations: readOnly,
		Description: "List the machines magi can send work to, each with its operating system, architecture and the tools it has (build toolchains, and Docker or Podman when they're running), its load read over SSH (cores, load average, memory and Claude sessions working there; from magi-overview, when installed, also CPU and Codex agents), whether it answers over SSH, magi tasks running on it, each host's budget and whether it has room, and the machine magi suggests. With magi-overview it also gives the Claude plan's usage and this machine's own load for comparison. When the work needs a platform or a tool, such as a Linux build from a Mac, pass needs: machines then ranks the places that can do it, best first: one that runs it natively, then a container on whichever machine has room, this machine included. Prefer a machine that runs it natively over a container or emulation here. Read-only. Call it before proposing where to run something, and tell the user which machine you'd pick and why.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in machinesInput) (*mcp.CallToolResult, any, error) {
		cfg, path, err := loadConfig()
		if err != nil {
			return nil, nil, err
		}
		if len(cfg.Hosts) == 0 {
			return nil, nil, fmt.Errorf("no hosts in %s; the user can add one with magi init", path)
		}
		return textResult(describeMachines(surveyFleet(ctx, cfg, true), in.Needs)), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "sessions",
		Annotations: readOnly,
		Description: "List the Claude Code sessions running on this machine and on every machine in the user's magi config, read over SSH: each one's id, title, whether it's busy, waiting (and on what) or idle, its folder, and its Remote Control link when it has one. Read-only.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, _ struct{}) (*mcp.CallToolResult, any, error) {
		all, err := everySession(ctx)
		if err != nil {
			return nil, nil, err
		}
		return textResult(describeSessions(all)), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "diff",
		Annotations: readOnly,
		Description: "Show a running Claude Code session's uncommitted changes (git diff against HEAD, plus new untracked files), on whichever machine it runs, read over SSH. Long diffs keep their end. Read-only.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in sessionQuery) (*mcp.CallToolResult, any, error) {
		all, err := everySession(ctx)
		if err != nil {
			return nil, nil, err
		}
		m, s, err := findAnySession(all, in.Session)
		if err != nil {
			return nil, nil, err
		}
		text, err := sessionDiff(m.Machine, s)
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "run_elsewhere",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Run a shell command, such as a slow test suite or a build, on another machine, in a copy of this project's exact working tree (commits, staged and unstaged changes, untracked and secret files). It copies the working tree first, usually in seconds, then returns a task id; the command runs in the background in a tab the user can watch, so long runs never block or time out. The copy is separate from any checkout on that machine and is removed when the result is collected; this session and this checkout are untouched. Dependencies such as node_modules aren't copied, but the project's approved .magi.toml setup runs first. Before calling: call machines, tell the user which machine you propose (the one machines suggests unless they named one) and the exact command, and wait for their OK. Afterwards run magi tasks wait <task id> as a background shell command to be told when it finishes, then task_result for the exit code and output.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in runInput) (*mcp.CallToolResult, any, error) {
		if strings.TrimSpace(in.Command) == "" {
			return nil, nil, fmt.Errorf("command is empty")
		}
		dir, err := projectDir(in.Dir)
		if err != nil {
			return nil, nil, err
		}
		text, err := startElsewhere(ctx, in.Host, TaskSpec{Kind: kindRun, Dir: dir, Command: in.Command, Setup: setupWanted(in.Setup)}, false)
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "delegate",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Give a self-contained task to a background Claude on another machine: a subagent that runs elsewhere, in its own copy of this project's working tree. It copies the working tree first, usually in seconds, then returns a task id. When it's done, task_result returns its final message, and applies its file changes to this working tree, keeping them on the branch magi/task-<id> too; with keep_branch they're only put on that branch. It runs headless (claude -p) in acceptEdits mode by default: it reads and edits files in its copy, and anything else that would ask for permission, such as shell commands or network access, is denied unless allowed_tools or that machine's own settings allow it. It never runs with permission checks bypassed. It isn't started when the user's Claude plan window is 90% used or more, since every machine shares that plan. It can't ask you questions, so put everything it needs in task. Before calling: call machines, then tell the user the machine you propose, what the task is, and any allowed_tools you want to grant, and wait for their OK.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in delegateInput) (*mcp.CallToolResult, any, error) {
		if strings.TrimSpace(in.Task) == "" {
			return nil, nil, fmt.Errorf("task is empty")
		}
		args, err := delegateArgs(in)
		if err != nil {
			return nil, nil, err
		}
		dir, err := projectDir(in.Dir)
		if err != nil {
			return nil, nil, err
		}
		text, err := startElsewhere(ctx, in.Host, TaskSpec{Kind: kindDelegate, Dir: dir, Prompt: in.Task, Claude: args, Setup: setupWanted(in.Setup), Keep: in.KeepBranch}, in.DespitePlan)
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "session_elsewhere",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Open a new interactive Claude session for this project on another machine, in its own copy of this project's working tree, in a Rex or tmux tab the user switches to, with Remote Control so it's also in every Claude app on their account. For when the user wants to work on something in parallel on a lighter machine. This session keeps running. It isn't opened when the user's Claude plan window is 90% used or more, since every machine shares that plan. Only with the user's clear yes: first call machines, then say which machine and what the new session will start with, and wait. When the user quits that session, task_result brings its changes back as a branch.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in sessionInput) (*mcp.CallToolResult, any, error) {
		dir, err := projectDir(in.Dir)
		if err != nil {
			return nil, nil, err
		}
		text, err := startElsewhere(ctx, in.Host, TaskSpec{Kind: kindSession, Dir: dir, Prompt: in.Prompt}, in.DespitePlan)
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "task_status",
		Annotations: readOnly,
		Description: "Check a task started with run_elsewhere, delegate or session_elsewhere: still running or finished, with its latest output. Without task_id, lists this machine's tasks. Read-only: it doesn't collect anything. Between checks, carry on with other work instead of polling in a loop.",
	}, func(_ context.Context, _ *mcp.CallToolRequest, in optionalTaskInput) (*mcp.CallToolResult, any, error) {
		if in.TaskID == "" {
			var b strings.Builder
			if err := printTasks(&b); err != nil {
				return nil, nil, err
			}
			return textResult(b.String()), nil, nil
		}
		r, err := taskStatus(in.TaskID)
		if err != nil {
			return nil, nil, err
		}
		return textResult(describeReport(r)), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "task_result",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no, IdempotentHint: true},
		Description: "Collect a finished task. A run_elsewhere task gives its exit code, the end of its output and the path of the full log on this machine. A delegate task gives Claude's final message and applies its changes to this working tree, keeping them on a branch too (only on the branch with keep_branch, or when they clash with changes made here since); a session_elsewhere task gives its changes as a branch. Collecting closes the task's tab and removes its copy on the other machine, except a finished delegate's copy: that stays for task_reply until cancel_task removes it or it has been idle for a day. If the task is still running, it says so and changes nothing. Calling it again returns the same result.",
	}, func(_ context.Context, _ *mcp.CallToolRequest, in taskInput) (*mcp.CallToolResult, any, error) {
		r, err := collectTask(in.TaskID, false)
		if err != nil {
			return nil, nil, err
		}
		return textResult(describeReport(r) + sweepIdleCopies()), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "task_reply",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Send a follow-up to a delegated Claude once its last turn has finished, for example \"now also fix the lint errors\". It resumes the same Claude session in the same copy on the same machine, so it keeps everything it knew and did; the follow-up runs as a new turn of the same task, with the same permissions. Returns at once; run magi tasks wait <task id> as a background shell command to be told when the turn is done, then collect it with task_result, which returns its new message and applies that turn's changes the same way. It isn't sent when the user's Claude plan window is 90% used or more, or when that machine's budget has no room. Tell the user what you're asking it before you send it.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in replyInput) (*mcp.CallToolResult, any, error) {
		data, _ := readFleet(ctx)
		heads, err := planGate(data.Plans, in.DespitePlan)
		if err != nil {
			return nil, nil, err
		}
		s, err := replyTask(ctx, in.TaskID, in.Message)
		if err != nil {
			return nil, nil, err
		}
		return textResult(heads + fmt.Sprintf("Sent the follow-up to task %s on %s as turn %d; it runs %s, tab %q. To be told when it's done, run `magi tasks wait %s` as a background shell command; then collect it with task_result.\n", s.Task.ID, s.Task.Host, s.Task.Turn, s.Task.Where, s.Task.tabLabel(), s.Task.ID)), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "cancel_task",
		Description: "Stop a running task, close its tab and remove its copy on the other machine; its unfinished changes are discarded. A run_elsewhere or delegate task's output so far is kept in a log on this machine; a session_elsewhere task keeps no output. Also use it to remove a finished delegate's copy once you're done sending it follow-ups. Ask the user before stopping a running task.",
	}, func(_ context.Context, _ *mcp.CallToolRequest, in taskInput) (*mcp.CallToolResult, any, error) {
		r, err := collectTask(in.TaskID, true)
		if err != nil {
			return nil, nil, err
		}
		return textResult(describeReport(r)), nil, nil
	})
	return server
}

func serveMCP(ctx context.Context) error {
	var err error
	if self, err = callerOf(ctx, os.Getppid()); err != nil {
		return err
	}
	server := newMCPServer()
	defer addOverviewTools(ctx, server)()
	return server.Run(ctx, &mcp.StdioTransport{})
}
