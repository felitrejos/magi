package main

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const serverInstructions = `cielago sends work from this session to the user's other machines over SSH: a shell command (run_elsewhere), a self-contained task for a background Claude (delegate), or a new interactive session (start_session). Each runs in its own copy of this project's working tree, in a Rex or tmux tab the user can watch, and never touches this session, this checkout or the user's checkouts there.

Starting work on another machine is the user's call. Before run_elsewhere, delegate or start_session: call machines, propose one machine (the least loaded unless the user named one), say exactly what will run there, and let the user choose: run it there, run it here instead, or don't run it. Wait for the user's clear answer. Then start it, carry on with other work, and check back with task_status; collect it with task_result once it's done. Don't poll in a tight loop.`

const delegateSystemPrompt = `You are a delegated task started by cielago from another machine. You work in a throwaway copy of the project on this machine; nobody is watching to answer questions or approve permission prompts. Tools that would need approval are denied: work around them, and say what you couldn't do. Work only inside this folder. Don't commit, push, open pull requests or change git configuration: leave your changes in the working tree, and cielago brings them back to the caller as a branch. Finish with a short report: what you did, which files you changed, how you checked it, and anything left undone.`

var modelPattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:\[\]-]*$`)

var delegateModes = []string{"acceptEdits", "dontAsk", "auto"}

type noInput struct{}

type runInput struct {
	Command string `json:"command" jsonschema:"the shell command to run from the project's folder, for example go test ./..."`
	Host    string `json:"host,omitempty" jsonschema:"the cielago host the user agreed to, as listed by machines; left out, the least loaded machine at this moment is used, which may not be the one you proposed"`
	Dir     string `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	Setup   *bool  `json:"setup,omitempty" jsonschema:"run the project's approved .cielago.toml setup first, such as installing dependencies (default true)"`
}

type delegateInput struct {
	Task           string   `json:"task" jsonschema:"a self-contained brief, since the remote Claude doesn't see this conversation and can't ask questions: the goal, the relevant files and what you already know about them, constraints, how to check the work, and what done looks like"`
	Host           string   `json:"host,omitempty" jsonschema:"the cielago host the user agreed to, as listed by machines; left out, the least loaded machine at this moment is used, which may not be the one you proposed"`
	Dir            string   `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	AllowedTools   []string `json:"allowed_tools,omitempty" jsonschema:"extra permission rules the remote Claude may use without asking, for example Bash(go test:*) or Bash(npm run lint); only what the user agreed to"`
	PermissionMode string   `json:"permission_mode,omitempty" jsonschema:"acceptEdits (default: read and edit files in its copy), dontAsk (only what allowed_tools and that machine's settings allow) or auto (Claude Code's auto mode, which reviews each action)"`
	Model          string   `json:"model,omitempty" jsonschema:"model for the remote Claude, such as sonnet or opus; leave out for that machine's default"`
	Setup          *bool    `json:"setup,omitempty" jsonschema:"run the project's approved .cielago.toml setup first (default true)"`
}

type sessionInput struct {
	Host   string `json:"host,omitempty" jsonschema:"the cielago host the user agreed to, as listed by machines; left out, the least loaded machine at this moment is used, which may not be the one you proposed"`
	Dir    string `json:"dir,omitempty" jsonschema:"absolute path of the project folder to copy; pass your current working directory"`
	Prompt string `json:"prompt,omitempty" jsonschema:"an optional first message for the new session"`
}

type taskInput struct {
	TaskID string `json:"task_id" jsonschema:"the task id a start tool returned"`
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
	args := []string{"-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode, "--permission-prompts", "none", "--no-session-persistence", "--append-system-prompt", delegateSystemPrompt}
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

func startElsewhere(ctx context.Context, host string, spec TaskSpec) (string, error) {
	c, err := chooseHost(ctx, host)
	if err != nil {
		return "", err
	}
	spec.Host = c.Host
	s, err := startTask(ctx, spec)
	if err != nil {
		return "", fmt.Errorf("starting it on %s failed: %w", c.Host, err)
	}
	return describeStarted(s, c.Why), nil
}

func newMCPServer() *mcp.Server {
	server := mcp.NewServer(&mcp.Implementation{Name: "cielago", Title: "cielago", Version: currentVersion()}, &mcp.ServerOptions{Instructions: serverInstructions})
	readOnly := &mcp.ToolAnnotations{ReadOnlyHint: true}
	no := false

	mcp.AddTool(server, &mcp.Tool{
		Name:        "machines",
		Annotations: readOnly,
		Description: "List the machines cielago can send work to, each with its live load from magi (CPU, load average, memory, and Claude or Codex agents working there), whether it answers over SSH, and cielago tasks running on it, plus this machine's own load for comparison and the machine cielago suggests. Read-only. Call it before proposing where to run something, and tell the user which machine you'd pick and why.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, _ noInput) (*mcp.CallToolResult, any, error) {
		cfg, path, err := loadConfig()
		if err != nil {
			return nil, nil, err
		}
		if len(cfg.Hosts) == 0 {
			return nil, nil, fmt.Errorf("no hosts in %s; the user can add one with cielago init", path)
		}
		return textResult(describeMachines(surveyFleet(ctx, cfg, true))), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "run_elsewhere",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Run a shell command, such as a slow test suite or a build, on another machine, in a copy of this project's exact working tree (commits, staged and unstaged changes, untracked and secret files). It copies the working tree first, usually in seconds, then returns a task id; the command runs in the background in a tab the user can watch, so long runs never block or time out. The copy is separate from any checkout on that machine and is removed when the result is collected; this session and this checkout are untouched. Dependencies such as node_modules aren't copied, but the project's approved .cielago.toml setup runs first. Before calling: call machines, tell the user which machine you propose (the least loaded unless they named one) and the exact command, and wait for their OK. Afterwards use task_status, then task_result for the exit code and output.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in runInput) (*mcp.CallToolResult, any, error) {
		if strings.TrimSpace(in.Command) == "" {
			return nil, nil, fmt.Errorf("command is empty")
		}
		dir, err := projectDir(in.Dir)
		if err != nil {
			return nil, nil, err
		}
		text, err := startElsewhere(ctx, in.Host, TaskSpec{Kind: kindRun, Dir: dir, Command: in.Command, Setup: setupWanted(in.Setup)})
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "delegate",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Give a self-contained task to a background Claude on another machine: a subagent that runs elsewhere, in its own copy of this project's working tree. It copies the working tree first, usually in seconds, then returns a task id. When it's done, task_result returns its final message, and its file changes as the branch cielago/task-<id> fetched into this repository without touching this working tree. It runs headless (claude -p) in acceptEdits mode by default: it reads and edits files in its copy, and anything else that would ask for permission, such as shell commands or network access, is denied unless allowed_tools or that machine's own settings allow it. It never runs with permission checks bypassed. It can't ask you questions, so put everything it needs in task. Before calling: call machines, then tell the user the machine you propose, what the task is, and any allowed_tools you want to grant, and wait for their OK.",
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
		text, err := startElsewhere(ctx, in.Host, TaskSpec{Kind: kindDelegate, Dir: dir, Prompt: in.Task, Claude: args, Setup: setupWanted(in.Setup)})
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "start_session",
		Annotations: &mcp.ToolAnnotations{DestructiveHint: &no},
		Description: "Open a new interactive Claude session for this project on another machine, in its own copy of this project's working tree, in a Rex or tmux tab the user switches to, with Remote Control so it's also in every Claude app on their account. For when the user wants to work on something in parallel on a lighter machine. This session keeps running. Only with the user's clear yes: first call machines, then say which machine and what the new session will start with, and wait. When the user quits that session, task_result brings its changes back as a branch.",
	}, func(ctx context.Context, _ *mcp.CallToolRequest, in sessionInput) (*mcp.CallToolResult, any, error) {
		dir, err := projectDir(in.Dir)
		if err != nil {
			return nil, nil, err
		}
		text, err := startElsewhere(ctx, in.Host, TaskSpec{Kind: kindSession, Dir: dir, Prompt: in.Prompt})
		if err != nil {
			return nil, nil, err
		}
		return textResult(text), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "task_status",
		Annotations: readOnly,
		Description: "Check a task started with run_elsewhere, delegate or start_session: still running or finished, with its latest output. Without task_id, lists this machine's tasks. Read-only: it doesn't collect anything. Between checks, carry on with other work instead of polling in a loop.",
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
		Description: "Collect a finished task. A run_elsewhere task gives its exit code, the end of its output and the path of the full log on this machine. A delegate task gives Claude's final message and its changes as a branch; a start_session task gives its changes as a branch. Collecting closes the task's tab and removes its copy on the other machine. If the task is still running, it says so and changes nothing. Calling it again returns the same result.",
	}, func(_ context.Context, _ *mcp.CallToolRequest, in taskInput) (*mcp.CallToolResult, any, error) {
		r, err := collectTask(in.TaskID, false)
		if err != nil {
			return nil, nil, err
		}
		return textResult(describeReport(r)), nil, nil
	})

	mcp.AddTool(server, &mcp.Tool{
		Name:        "cancel_task",
		Description: "Stop a running task, close its tab and remove its copy on the other machine. Its changes are discarded; the output so far is kept in a log on this machine. Ask the user before cancelling a task.",
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
	return newMCPServer().Run(ctx, &mcp.StdioTransport{})
}
