package main

import (
	"context"
	"os/exec"
	"slices"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const overviewInstructions = `

Use sessions to see the Claude Code sessions running on the user's machines and diff to see one's uncommitted changes; both read the machines over SSH. When magi-overview is installed and its agents run (Rex and Tailscale), you also get recent_work, subagents and wait to follow a session's work, alerts for what needs the user, and send_message, answer_permission, choose_option, move_session and start_session, which type into the user's live sessions or start new ones: say what you'll do and let the user decide first. If the overview isn't running, those tools say so; everything above still works.`

func addOverviewTools(ctx context.Context, server *mcp.Server) func() {
	path, err := exec.LookPath("magi-overview")
	if err != nil {
		return func() {}
	}
	client := mcp.NewClient(&mcp.Implementation{Name: "magi", Version: currentVersion()}, nil)
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
		if slices.Contains([]string{"machines", "sessions", "diff"}, tool.Name) {
			continue
		}
		name := tool.Name
		server.AddTool(tool, func(ctx context.Context, req *mcp.CallToolRequest) (*mcp.CallToolResult, error) {
			return session.CallTool(ctx, &mcp.CallToolParams{Name: name, Arguments: req.Params.Arguments})
		})
	}
	return func() { session.Close() }
}
