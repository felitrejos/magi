package main

import (
	"context"
	"os/exec"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const overviewInstructions = `

When magi's overview runs on the user's machines (a magi agent on each, over Tailscale), you also see every Claude Code and Codex session on them. Use sessions to look, recent_work and diff to see what a session did, subagents and wait to follow work, and alerts for what needs the user. send_message, answer_permission, choose_option, move_session and start_session type into the user's live sessions or start new ones: say what you'll do and let the user decide first. If the overview isn't running, those tools say so; everything above still works.`

func addOverviewTools(ctx context.Context, server *mcp.Server) func() {
	path, err := exec.LookPath("magi")
	if err != nil {
		return func() {}
	}
	client := mcp.NewClient(&mcp.Implementation{Name: "magi-remote", Version: currentVersion()}, nil)
	session, err := client.Connect(ctx, &mcp.CommandTransport{Command: exec.Command(path, "mcp-overview")}, nil)
	if err != nil {
		return func() {}
	}
	listed, err := session.ListTools(ctx, nil)
	if err != nil {
		session.Close()
		return func() {}
	}
	for _, tool := range listed.Tools {
		if tool.Name == "machines" {
			continue
		}
		name := tool.Name
		server.AddTool(tool, func(ctx context.Context, req *mcp.CallToolRequest) (*mcp.CallToolResult, error) {
			return session.CallTool(ctx, &mcp.CallToolParams{Name: name, Arguments: req.Params.Arguments})
		})
	}
	return func() { session.Close() }
}
