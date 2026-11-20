package main

import (
	"context"
	"fmt"
	"os"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

var version = "dev"

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
		fmt.Println("magi", version)
	default:
		fmt.Fprintln(os.Stderr, "magi runs through its Claude Code plugin; ask Claude for what you need")
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "magi:", err)
		os.Exit(1)
	}
}
