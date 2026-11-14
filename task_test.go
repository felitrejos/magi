package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

func (f *fixture) taskHost(extra string) {
	f.t.Helper()
	f.isolatedTmux()
	f.config("local_roots = [\"~/laptop\"]\nexclude = [\"node_modules\"]\n" + extra + `
[hosts.server]
ssh = "fakehost"
claude = "~/bin/claude"
roots = ["~/developer"]
secrets = [".env", "secret.txt"]
launcher = "tmux"
`)
}

func (f *fixture) waitForTask(id string) *taskReport {
	f.t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for {
		r, err := taskStatus(id)
		if err != nil {
			f.t.Fatal(err)
		}
		if r.State.State != "running" {
			return r
		}
		if time.Now().After(deadline) {
			f.t.Fatalf("task %s still running: %s", id, describeReport(r))
		}
		time.Sleep(100 * time.Millisecond)
	}
}

func (f *fixture) untouched(before string) {
	f.t.Helper()
	if after := f.git(f.local, "status", "--porcelain"); after != before {
		f.t.Errorf("the local checkout changed:\n%s\nwas:\n%s", after, before)
	}
	if ref := f.git(f.local, "for-each-ref", "refs/worktree", "refs/cielago"); ref != "" {
		f.t.Errorf("the local checkout got refs: %s", ref)
	}
	if status := f.git(f.remote, "status", "--porcelain"); status != "" {
		f.t.Errorf("the server's own checkout changed:\n%s", status)
	}
	if ref := f.git(f.remote, "for-each-ref", "refs/cielago", "refs/worktree", "refs/heads/cielago"); ref != "" {
		f.t.Errorf("the server's own checkout got refs: %s", ref)
	}
}

func TestARunElsewhereTaskRunsOnACopyOfTheWorkingTreeAndReturnsItsOutput(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.messUp(f.local)
	before := f.git(f.local, "status", "--porcelain")
	if out, err := exec.Command("tmux", "new-session", "-d", "-s", "acme").CombinedOutput(); err != nil {
		t.Fatalf("tmux: %v %s", err, out)
	}

	s, err := startTask(context.Background(), TaskSpec{Kind: kindRun, Host: "server", Dir: f.local, Command: `cat edited.txt; echo; cat .env; echo; git status --porcelain; ls node_modules 2>&1; exit 3`})
	if err != nil {
		t.Fatal(err)
	}
	task := s.Task
	if !strings.HasPrefix(task.Checkout, f.home+"/.cielago/tasks/"+task.ID+"/") {
		t.Errorf("checkout at %s", task.Checkout)
	}
	windows, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{session_name} #{window_name} #{window_active}").Output()
	if !strings.Contains(string(windows), "acme api · run "+task.ID+" 0") {
		t.Errorf("tmux windows = %q, want an inactive task tab in the project's session", windows)
	}

	if r := f.waitForTask(task.ID); r.State.State != "done" || r.State.Exit != "exit 3" {
		t.Fatalf("status = %+v", r.State)
	}
	r, err := collectTask(task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	o := r.Task.Outcome
	if o == nil || o.Exit != "exit 3" {
		t.Fatalf("outcome = %+v", o)
	}
	for _, want := range []string{"staged then edited", "SECRET=1", "MM edited.txt", `?? "dir with space/"`, "No such file"} {
		if !strings.Contains(o.Tail, want) {
			t.Errorf("output lacks %q:\n%s", want, o.Tail)
		}
	}
	if log := f.read(filepath.Dir(o.Log), filepath.Base(o.Log)); !strings.Contains(log, "cielago: running:") {
		t.Errorf("log = %q", log)
	}
	if _, err := os.Stat(task.Dir); !os.IsNotExist(err) {
		t.Errorf("the task's copy is still there: %v", err)
	}
	if out, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{window_name}").Output(); strings.Contains(string(out), task.ID) {
		t.Errorf("the task's tab is still open: %s", out)
	}
	f.untouched(before)
	if again, err := collectTask(task.ID, false); err != nil || again.Task.Outcome.Exit != "exit 3" {
		t.Errorf("collecting again = %+v, %v", again, err)
	}
	if text := describeReport(r); !strings.Contains(text, "exit 3") || !strings.Contains(text, o.Log) {
		t.Errorf("report:\n%s", text)
	}
}

const fakeDelegateClaude = `#!/bin/sh
prompt=$(cat)
printf '%s\n' "$@" > "$HOME/claude-args"
printf '{"type":"system","subtype":"init"}\n'
printf 'claude was here\n' > claude.txt
printf 'changed by claude\n' > kept.txt
rm -f committed.txt
printf '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"total_cost_usd":0.12,"result":"Did: %s"}\n' "$prompt"
`

func TestADelegatedTaskReturnsItsMessageAndItsChangesAsABranch(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", fakeDelegateClaude)
	f.messUp(f.local)
	before := f.git(f.local, "status", "--porcelain")
	args, err := delegateArgs(delegateInput{AllowedTools: []string{"Bash(go test:*)"}})
	if err != nil {
		t.Fatal(err)
	}
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "tidy up", Claude: args})
	if err != nil {
		t.Fatal(err)
	}
	id := s.Task.ID
	f.waitForTask(id)
	r, err := collectTask(id, false)
	if err != nil {
		t.Fatal(err)
	}
	o := r.Task.Outcome
	if !strings.HasPrefix(o.Message, "Did: tidy up") || !strings.Contains(o.Message, "3 turns") {
		t.Errorf("message = %q", o.Message)
	}
	if o.Branch != "cielago/task-"+id {
		t.Fatalf("branch = %q (note %q)", o.Branch, o.Note)
	}
	changed := f.git(f.local, "diff", "--name-status", s.Task.Base, o.Branch)
	if changed != "A\tclaude.txt\nD\tcommitted.txt\nM\tkept.txt" {
		t.Errorf("the branch changes:\n%s", changed)
	}
	if parent := f.git(f.local, "rev-parse", o.Branch+"^"); parent != s.Task.Base {
		t.Errorf("the branch's parent is %s, want the working tree snapshot %s", parent, s.Task.Base)
	}
	if got := f.read(f.local, "kept.txt"); got != "v1" {
		t.Errorf("kept.txt here = %q", got)
	}
	f.untouched(before)
	claudeArgs := f.read(f.home, "claude-args")
	for _, want := range []string{"-p", "acceptEdits", "--permission-prompts\nnone", "--allowedTools\nBash(go test:*)"} {
		if !strings.Contains(claudeArgs, want) {
			t.Errorf("claude args lack %q:\n%s", want, claudeArgs)
		}
	}
	if strings.Contains(claudeArgs, "bypass") || strings.Contains(claudeArgs, "dangerously") {
		t.Errorf("claude ran with permissions bypassed:\n%s", claudeArgs)
	}
	if text := describeReport(r); !strings.Contains(text, "git diff "+shortCommit(s.Task.Base)+" "+o.Branch) {
		t.Errorf("report:\n%s", text)
	}
}

func TestADelegatedTaskThatChangesNothingSaysSo(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\necho '{\"type\":\"result\",\"subtype\":\"success\",\"num_turns\":1,\"result\":\"nothing to do\"}'\n")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "look"})
	if err != nil {
		t.Fatal(err)
	}
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	if r.Task.Outcome.Branch != "" || !strings.Contains(describeReport(r), "It changed no files.") {
		t.Errorf("report:\n%s", describeReport(r))
	}
}

func TestCancellingATaskStopsItAndRemovesItsCopy(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	marker := filepath.Join(f.home, "still-running")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindRun, Host: "server", Dir: f.local, Command: "sleep 30 && touch " + quote(marker)})
	if err != nil {
		t.Fatal(err)
	}
	if r, err := taskStatus(s.Task.ID); err != nil || r.State.State != "running" {
		t.Fatalf("status = %+v, %v", r, err)
	}
	r, err := collectTask(s.Task.ID, false)
	if err != nil || r.Task.Outcome != nil || r.State.State != "running" {
		t.Fatalf("collecting a running task = %+v, %v", r, err)
	}
	r, err = collectTask(s.Task.ID, true)
	if err != nil {
		t.Fatal(err)
	}
	if r.Task.Outcome == nil || r.Task.Outcome.State != "cancelled" {
		t.Fatalf("outcome = %+v", r.Task.Outcome)
	}
	if _, err := os.Stat(s.Task.Dir); !os.IsNotExist(err) {
		t.Errorf("the task's copy is still there: %v", err)
	}
	if out, _ := exec.Command("pgrep", "-f", "touch "+marker).Output(); len(out) > 0 {
		t.Errorf("the command is still running: %s", out)
	}
}

func TestAPlainFolderTaskBringsTheFolderBack(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	notes := filepath.Join(f.home, "laptop", "notes")
	f.write(notes, "todo.md", "one")
	f.write(notes, "node_modules/x", "skip")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nprintf two > todo.md\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}'\n")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: notes, Prompt: "edit"})
	if err != nil {
		t.Fatal(err)
	}
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	files := r.Task.Outcome.Files
	if got := f.read(files, "todo.md"); got != "two" {
		t.Errorf("todo.md back here = %q", got)
	}
	if got := f.read(notes, "todo.md"); got != "one" {
		t.Errorf("the original folder changed: %q", got)
	}
	if got := f.read(files, "node_modules/x"); got != "<missing>" {
		t.Errorf("an excluded folder was copied: %q", got)
	}
}

const fakeMagi = `#!/bin/sh
[ "$1 $2" = "fleet --json" ] || exit 2
cat <<'EOF'
{"machines":[
 {"name":"laptop","here":true,"live":true,"os":"macOS","cores":10,"cpu":0.9,"load":[9,8,7],"memory_used":1,"memory_total":2,"agents":{"working":3,"waiting":0,"idle":0},"error":null},
 {"name":"homelab","here":false,"live":true,"os":"Ubuntu","cores":12,"cpu":0.4,"load":[1,1,1],"memory_used":1,"memory_total":4,"agents":{"working":1,"waiting":0,"idle":2},"error":null,"future":"ignored"},
 {"name":"box.tail1234.ts.net","here":false,"live":true,"os":"Ubuntu","cores":4,"cpu":0.05,"load":[0.1,0,0],"memory_used":1,"memory_total":4,"agents":{"working":0,"waiting":0,"idle":0},"error":null},
 {"name":"gpu","here":false,"live":false,"cores":0,"cpu":0,"load":[],"memory_used":0,"memory_total":0,"agents":{"working":0,"waiting":0,"idle":0},"error":"connection refused"},
 {"name":"stranger","here":false,"live":true,"os":"Ubuntu","cores":4,"cpu":0.0,"load":[0,0,0],"memory_used":1,"memory_total":4,"agents":{"working":0,"waiting":0,"idle":0},"error":null}
]}
EOF
`

const threeHosts = `
[hosts.homelab]
ssh = "homelab"
claude = "claude"
roots = ["~/developer"]
launcher = "tmux"

[hosts.small]
ssh = "me@box"
claude = "claude"
roots = ["~/developer"]
launcher = "tmux"

[hosts.gpu]
ssh = "gpu"
claude = "claude"
roots = ["~/developer"]
launcher = "tmux"
`

func TestTheLeastLoadedMachineIsChosenByNameOrSSHDestination(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	f.tool("magi", fakeMagi)
	c, err := chooseHost(context.Background(), "")
	if err != nil {
		t.Fatal(err)
	}
	if c.Host != "small" || !strings.Contains(c.Why, "cpu 5%") {
		t.Errorf("chose %+v, want small", c)
	}
	if c, err := chooseHost(context.Background(), "homelab"); err != nil || c.Host != "homelab" {
		t.Errorf("asking for homelab gave %+v, %v", c, err)
	}
	if _, err := chooseHost(context.Background(), "nowhere"); err == nil || !strings.Contains(err.Error(), "configured hosts: gpu, homelab, small") {
		t.Errorf("asking for an unknown host: %v", err)
	}
	cfg, _, _ := loadConfig()
	text := describeMachines(surveyFleet(context.Background(), cfg, true))
	for _, want := range []string{"This machine (laptop): cpu 90%", "- gpu (ssh gpu): reachable; magi: not answering (connection refused)", "- small (ssh me@box): reachable; Ubuntu cpu 5%", "magi also sees stranger"} {
		if !strings.Contains(text, want) {
			t.Errorf("machines lacks %q:\n%s", want, text)
		}
	}
}

func TestWithoutMagiTheFirstReachableHostIsChosenAndTheReasonGiven(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	t.Setenv("PATH", filepath.Join(f.home, "bin")+":/usr/bin:/bin")
	c, err := chooseHost(context.Background(), "")
	if err != nil {
		t.Fatal(err)
	}
	if c.Host != "gpu" || !strings.Contains(c.Why, "magi isn't installed") {
		t.Errorf("chose %+v", c)
	}
	f.tool("magi", "#!/bin/sh\necho '{\"machines\":[]}'\n")
	if c, err := chooseHost(context.Background(), ""); err != nil || !strings.Contains(c.Why, "listed no machines") {
		t.Errorf("with an empty fleet: %+v, %v", c, err)
	}
}

func TestADelegatedClaudeNeverBypassesPermissions(t *testing.T) {
	args, err := delegateArgs(delegateInput{})
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(args[:9], " "); got != "-p --output-format stream-json --verbose --permission-mode acceptEdits --permission-prompts none --no-session-persistence" {
		t.Errorf("args = %s", got)
	}
	for _, mode := range []string{"bypassPermissions", "default", "plan"} {
		if _, err := delegateArgs(delegateInput{PermissionMode: mode}); err == nil {
			t.Errorf("permission mode %s was accepted", mode)
		}
	}
	if _, err := delegateArgs(delegateInput{AllowedTools: []string{"--dangerously-skip-permissions"}}); err == nil {
		t.Error("a flag was accepted as an allowed tool")
	}
}

func TestTheToolServerOffersItsTools(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	f.tool("magi", fakeMagi)
	serverSide, clientSide := mcp.NewInMemoryTransports()
	ctx := context.Background()
	if _, err := newMCPServer().Connect(ctx, serverSide, nil); err != nil {
		t.Fatal(err)
	}
	session, err := mcp.NewClient(&mcp.Implementation{Name: "test", Version: "1"}, nil).Connect(ctx, clientSide, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer session.Close()
	if !strings.Contains(session.InitializeResult().Instructions, "run it here instead, or don't run it") {
		t.Errorf("instructions = %q", session.InitializeResult().Instructions)
	}
	tools, err := session.ListTools(ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, tool := range tools.Tools {
		names = append(names, tool.Name)
	}
	if got := strings.Join(names, " "); got != "cancel_task delegate machines run_elsewhere start_session task_result task_status" {
		t.Errorf("tools = %s", got)
	}
	res, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "machines"})
	if err != nil || res.IsError {
		t.Fatalf("machines = %+v, %v", res, err)
	}
	if text := res.Content[0].(*mcp.TextContent).Text; !strings.Contains(text, "- small (ssh me@box)") {
		t.Errorf("machines:\n%s", text)
	}
	res, err = session.CallTool(ctx, &mcp.CallToolParams{Name: "delegate", Arguments: map[string]any{"task": "x", "permission_mode": "bypassPermissions"}})
	if err != nil || !res.IsError || !strings.Contains(res.Content[0].(*mcp.TextContent).Text, "isn't allowed") {
		t.Errorf("delegate with bypassPermissions = %+v, %v", res, err)
	}
	res, err = session.CallTool(ctx, &mcp.CallToolParams{Name: "task_result", Arguments: map[string]any{"task_id": "nope"}})
	if err != nil || !res.IsError {
		t.Errorf("task_result for a bad id = %+v, %v", res, err)
	}
}

func TestASessionTaskBringsItsChangesBackOnceTheUserQuits(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/claude-args\"\nwhile [ ! -f \"$HOME/quit\" ]; do sleep 0.1; done\nprintf done > session.txt\n")
	f.write(f.home, ".claude.json", `{"projects":{}}`)
	s, err := startTask(context.Background(), TaskSpec{Kind: kindSession, Host: "server", Dir: f.local, Prompt: "hello there"})
	if err != nil {
		t.Fatal(err)
	}
	if text := describeStarted(s, "test"); !strings.Contains(text, "trust the copy's folder") {
		t.Errorf("start report doesn't mention the trust prompt:\n%s", text)
	}
	if !waitForFile(t, filepath.Join(f.home, "claude-args"), 10*time.Second) {
		t.Fatal("claude didn't start")
	}
	if got := f.read(f.home, "claude-args"); got != "--remote-control\napi task "+s.Task.ID+"\n--\nhello there\n" {
		t.Errorf("claude args = %q", got)
	}
	if r, err := collectTask(s.Task.ID, false); err != nil || r.Task.Outcome != nil {
		t.Fatalf("a live session was collected: %+v, %v", r, err)
	}
	f.write(f.home, "quit", "")
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	if got := f.git(f.local, "diff", "--name-only", s.Task.Base, r.Task.Outcome.Branch); got != "session.txt" {
		t.Errorf("the branch changes %q; report:\n%s", got, describeReport(r))
	}
}

func TestASecretIgnoredOnlyHereNeverReachesTheResultBranch(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nprintf changed > kept.txt\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}'\n")
	f.write(filepath.Join(f.local, ".git", "info"), "exclude", "secret.txt\n")
	f.write(f.local, "secret.txt", "hunter2")
	f.write(f.local, "untracked.txt", "mine")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "x"})
	if err != nil {
		t.Fatal(err)
	}
	if got := f.read(s.Task.Checkout, "secret.txt"); got != "hunter2" {
		t.Fatalf("the secret didn't reach the copy: %q", got)
	}
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	if got := f.git(f.local, "diff", "--name-status", s.Task.Base, r.Task.Outcome.Branch); got != "M\tkept.txt" {
		t.Errorf("the branch changes:\n%s", got)
	}
	if files := f.git(f.local, "ls-tree", "-r", "--name-only", r.Task.Outcome.Branch); strings.Contains(files, "secret.txt") {
		t.Errorf("the secret is on the branch:\n%s", files)
	}
}

func TestCancellingAFinishedTaskStillBringsItsChangesBack(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nprintf changed > kept.txt\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}'\n")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "x"})
	if err != nil {
		t.Fatal(err)
	}
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, true)
	if err != nil {
		t.Fatal(err)
	}
	if o := r.Task.Outcome; o.State == "cancelled" || o.Branch == "" {
		t.Errorf("outcome = %+v", o)
	}
}

func TestACollectThatCannotFetchKeepsTheCopyForARetry(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nprintf changed > kept.txt\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}'\n")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "x"})
	if err != nil {
		t.Fatal(err)
	}
	f.waitForTask(s.Task.ID)
	moved := f.local + "-moved"
	if err := os.Rename(f.local, moved); err != nil {
		t.Fatal(err)
	}
	if _, err := collectTask(s.Task.ID, false); err == nil || !strings.Contains(err.Error(), "was kept") {
		t.Fatalf("collect = %v", err)
	}
	if _, err := os.Stat(s.Task.Checkout); err != nil {
		t.Fatalf("the copy is gone: %v", err)
	}
	if err := os.Rename(moved, f.local); err != nil {
		t.Fatal(err)
	}
	r, err := collectTask(s.Task.ID, false)
	if err != nil || r.Task.Outcome.Branch == "" {
		t.Fatalf("retry = %+v, %v", r, err)
	}
}

func TestTaskScriptsOnlyTouchTaskFolders(t *testing.T) {
	f := newFixture(t)
	for _, dir := range []string{f.home + "/.cielago/tasks/..", f.home + "/.cielago/tasks/abc", f.home + "/elsewhere/0123abcd", f.home + "/.cielago/tasks/0123abcd/.."} {
		if _, err := sh(f.machine(), taskScript, "remove", dir); err == nil {
			t.Errorf("remove accepted %s", dir)
		}
	}
}
