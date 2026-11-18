package main

import (
	"context"
	"encoding/json"
	"errors"
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
	f.serverUntouched()
}

func (f *fixture) serverUntouched() {
	f.t.Helper()
	if ref := f.git(f.local, "for-each-ref", "refs/worktree", "refs/magi"); ref != "" {
		f.t.Errorf("the local checkout got refs: %s", ref)
	}
	if status := f.git(f.remote, "status", "--porcelain"); status != "" {
		f.t.Errorf("the server's own checkout changed:\n%s", status)
	}
	if ref := f.git(f.remote, "for-each-ref", "refs/magi", "refs/worktree", "refs/heads/magi"); ref != "" {
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
	if !strings.HasPrefix(task.Checkout, f.home+"/.magi/tasks/"+task.ID+"/") {
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
	if log := f.read(filepath.Dir(o.Log), filepath.Base(o.Log)); !strings.Contains(log, "magi: running:") {
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

func TestADelegatedTaskAppliesItsChangesHereAndKeepsThemOnABranch(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", fakeDelegateClaude)
	f.messUp(f.local)
	staged := f.git(f.local, "diff", "--cached", "--name-status")
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
	if o.Branch != "magi/task-"+id {
		t.Fatalf("branch = %q (note %q)", o.Branch, o.Note)
	}
	changed := f.git(f.local, "diff", "--name-status", s.Task.Base, o.Branch)
	if changed != "A\tclaude.txt\nD\tcommitted.txt\nM\tkept.txt" {
		t.Errorf("the branch changes:\n%s", changed)
	}
	if parent := f.git(f.local, "rev-parse", o.Branch+"^"); parent != s.Task.Base {
		t.Errorf("the branch's parent is %s, want the working tree snapshot %s", parent, s.Task.Base)
	}
	if !o.Applied || o.Unapplied != "" {
		t.Fatalf("applied = %v, unapplied = %q", o.Applied, o.Unapplied)
	}
	if got := f.read(f.local, "kept.txt"); got != "changed by claude\n" {
		t.Errorf("kept.txt here = %q", got)
	}
	if got := f.read(f.local, "claude.txt"); got != "claude was here\n" {
		t.Errorf("claude.txt here = %q", got)
	}
	if _, err := os.Stat(filepath.Join(f.local, "committed.txt")); !os.IsNotExist(err) {
		t.Errorf("committed.txt is still here: %v", err)
	}
	if got := f.read(f.local, "edited.txt"); got != "staged then edited" {
		t.Errorf("the user's own edit changed: %q", got)
	}
	if got := f.git(f.local, "diff", "--cached", "--name-status"); got != staged {
		t.Errorf("what's staged changed:\n%s\nwas:\n%s", got, staged)
	}
	f.serverUntouched()
	claudeArgs := f.read(f.home, "claude-args")
	for _, want := range []string{"-p", "acceptEdits", "--permission-prompts\nnone", "--allowedTools\nBash(go test:*)"} {
		if !strings.Contains(claudeArgs, want) {
			t.Errorf("claude args lack %q:\n%s", want, claudeArgs)
		}
	}
	if strings.Contains(claudeArgs, "bypass") || strings.Contains(claudeArgs, "dangerously") {
		t.Errorf("claude ran with permissions bypassed:\n%s", claudeArgs)
	}
	if text := describeReport(r); !strings.Contains(text, "have been applied to your working tree") || !strings.Contains(text, "git diff "+shortCommit(s.Task.Base)+" "+o.Branch) {
		t.Errorf("report:\n%s", text)
	}
	if again, err := collectTask(id, false); err != nil || f.read(f.local, "kept.txt") != "changed by claude\n" || !again.Task.Outcome.Applied {
		t.Errorf("collecting again = %v", err)
	}
}

func TestADelegatedTaskAskedToKeepItsBranchLeavesTheWorkingTreeAlone(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", fakeDelegateClaude)
	f.messUp(f.local)
	before := f.git(f.local, "status", "--porcelain")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "tidy up", Keep: true})
	if err != nil {
		t.Fatal(err)
	}
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	if o := r.Task.Outcome; o.Applied || o.Branch == "" {
		t.Fatalf("outcome = %+v", o)
	}
	f.untouched(before)
	if text := describeReport(r); !strings.Contains(text, "your working tree wasn't touched") || !strings.Contains(text, "| git apply") {
		t.Errorf("report:\n%s", text)
	}
}

func TestChangesThatClashWithEditsMadeHereSinceAreLeftOnTheBranch(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nsleep 1\nprintf 'theirs\\n' > kept.txt\nprintf new > added.txt\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}'\n")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "x"})
	if err != nil {
		t.Fatal(err)
	}
	f.write(f.local, "kept.txt", "mine\n")
	f.waitForTask(s.Task.ID)
	r, err := collectTask(s.Task.ID, false)
	if err != nil {
		t.Fatal(err)
	}
	o := r.Task.Outcome
	if o.Applied || !strings.Contains(o.Unapplied, "kept.txt") || o.Branch == "" {
		t.Fatalf("outcome = %+v", o)
	}
	if got := f.read(f.local, "kept.txt"); got != "mine\n" {
		t.Errorf("kept.txt here = %q", got)
	}
	if _, err := os.Stat(filepath.Join(f.local, "added.txt")); !os.IsNotExist(err) {
		t.Errorf("part of the changes was applied: %v", err)
	}
	if text := describeReport(r); !strings.Contains(text, "couldn't be applied") || !strings.Contains(text, "git apply --3way") {
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

func (f *fixture) probesAs(output string) {
	f.t.Helper()
	saved := probeMachine
	probeMachine = func(_ context.Context, name string, _ Host) (*fleetMachine, error) {
		return parsePlatform(name, output), nil
	}
	f.t.Cleanup(func() { probeMachine = saved })
}

const quietLinux = "Ubuntu 24.04\nx86_64\ndocker\n4\n0.10 0.10 0.10\n8000000000 6000000000\n0\n"

func TestTheFirstMachineInConfigOrderWithRoomIsChosen(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	f.probesAs(quietLinux)
	f.tool("magi-overview", fakeMagi)
	picks, skipped, _, err := chooseHosts(context.Background(), "", true)
	if err != nil {
		t.Fatal(err)
	}
	if len(picks) != 3 || picks[0].Host != "homelab" || picks[1].Host != "small" || picks[2].Host != "gpu" || len(skipped) != 0 {
		t.Fatalf("picks = %+v, skipped %v", picks, skipped)
	}
	if !strings.Contains(picks[0].Why, "first machine in your list with room") || !strings.Contains(picks[2].Why, "read over SSH") {
		t.Errorf("reasons = %+v", picks)
	}
	if picks, _, _, err := chooseHosts(context.Background(), "gpu", true); err != nil || len(picks) != 1 || picks[0].Host != "gpu" {
		t.Errorf("asking for gpu gave %+v, %v", picks, err)
	}
	if _, _, _, err := chooseHosts(context.Background(), "nowhere", true); err == nil || !strings.Contains(err.Error(), "configured hosts: gpu, homelab, small") {
		t.Errorf("asking for an unknown host: %v", err)
	}

	f.config(strings.Replace(threeHosts, "[hosts.homelab]\n", "[hosts.homelab]\nmax_agents = 1\n", 1))
	picks, skipped, _, _ = chooseHosts(context.Background(), "", true)
	if picks[0].Host != "small" || len(skipped) != 1 || !strings.Contains(skipped[0], "homelab has no room: 1 agents are working there, its budget is 1") {
		t.Errorf("with homelab full: picks %+v, skipped %v", picks, skipped)
	}
	if picks, _, _, _ := chooseHosts(context.Background(), "", false); picks[0].Host != "homelab" {
		t.Errorf("a command isn't an agent, so homelab still has room: %+v", picks)
	}

	cfg, _, _ := loadConfig()
	text := describeMachines(surveyFleet(context.Background(), cfg, true), needsInput{})
	for _, want := range []string{
		"This machine (laptop): cpu 90%",
		"- homelab (ssh homelab): reachable; budget: at most 1 working agents; NO ROOM now: 1 agents are working there",
		"- gpu (ssh gpu): reachable; budget: no budget; ",
		"Claude sessions working, read over SSH",
		"- small (ssh me@box): reachable; budget: no budget; Ubuntu cpu 5%",
		"Suggested: small,",
		"magi also sees stranger",
	} {
		if !strings.Contains(text, want) {
			t.Errorf("machines lacks %q:\n%s", want, text)
		}
	}
	if strings.Index(text, "- homelab") > strings.Index(text, "- small") || strings.Index(text, "- small") > strings.Index(text, "- gpu") {
		t.Errorf("machines aren't in config order:\n%s", text)
	}
}

func TestWithoutTheOverviewMachinesAreTriedInConfigOrderAndTheReasonGiven(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	f.probesAs("Ubuntu 24.04\nx86_64\n\n")
	t.Setenv("PATH", filepath.Join(f.home, "bin")+":/usr/bin:/bin")
	picks, _, _, err := chooseHosts(context.Background(), "", true)
	if err != nil {
		t.Fatal(err)
	}
	if picks[0].Host != "homelab" || !strings.Contains(picks[0].Why, "magi-overview isn't installed") {
		t.Errorf("picks = %+v", picks)
	}
	f.tool("magi-overview", "#!/bin/sh\necho '{\"machines\":[]}'\n")
	if picks, _, _, err := chooseHosts(context.Background(), "", true); err != nil || !strings.Contains(picks[0].Why, "listed no machines") {
		t.Errorf("with an empty fleet: %+v, %v", picks, err)
	}
}

func TestBudgetsParseMemoryAndQuietHours(t *testing.T) {
	for text, want := range map[string]float64{"4G": 4 << 30, "512M": 512 << 20, "1.5GB": 1.5 * (1 << 30), "2 GiB": 2 << 30, "100": 100} {
		if got, err := parseMemory(text); err != nil || got != want {
			t.Errorf("parseMemory(%q) = %v, %v; want %v", text, got, err, want)
		}
	}
	if _, err := parseMemory("lots"); err == nil {
		t.Error("parseMemory accepted lots")
	}
	b, err := Host{MaxAgents: 2, MinFreeMemory: "4G", QuietHours: "22:00-07:00"}.budget()
	if err != nil {
		t.Fatal(err)
	}
	at := func(clock string) time.Time {
		c, _ := time.Parse("15:04", clock)
		return time.Date(2026, 11, 15, c.Hour(), c.Minute(), 0, 0, time.Local)
	}
	for clock, quiet := range map[string]bool{"21:59": false, "22:00": true, "03:00": true, "06:59": true, "07:00": false, "12:00": false} {
		if got := b.quietAt(at(clock)); got != quiet {
			t.Errorf("quiet at %s = %v", clock, got)
		}
	}
	if got := b.String(); got != "at most 2 working agents, at least 4.0 GB free memory, nothing new 22:00-07:00" {
		t.Errorf("budget = %q", got)
	}
	tight := &fleetMachine{Live: true, MemoryTotal: 8 << 30, MemoryUsed: 6 << 30, Cores: 4, Load: []float64{0}}
	if got := b.noRoom(at("12:00"), tight, true); !strings.Contains(got, "2.0 GB of memory is free there, its budget needs 4.0 GB") {
		t.Errorf("noRoom = %q", got)
	}
	if err := (Host{SSH: "x", Claude: "c", Roots: []string{"~/d"}, Launcher: "tmux", QuietHours: "late"}).validate("x"); err == nil {
		t.Error("a bad quiet_hours was accepted")
	}
}

func TestTheTargetRechecksItsBudgetWhenATaskStarts(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nsleep 30\n")
	f.config(strings.Replace(f.read(filepath.Join(f.home, ".config", "magi"), "config.toml"), "launcher = \"tmux\"", "launcher = \"tmux\"\nmax_agents = 1", 1))
	first, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "x"})
	if err != nil {
		t.Fatal(err)
	}
	_, err = startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "y"})
	var full *noRoomError
	if !errors.As(err, &full) || !strings.HasPrefix(full.Reason, "1 agents are already running there") {
		t.Fatalf("second delegate = %v", err)
	}
	if _, err := startTask(context.Background(), TaskSpec{Kind: kindRun, Host: "server", Dir: f.local, Command: "true"}); err != nil {
		t.Errorf("a command was refused: %v", err)
	}
	tasks, _ := listTasks()
	if len(tasks) != 2 {
		t.Errorf("%d task records, want the delegate and the command", len(tasks))
	}
	collectTask(first.Task.ID, true)
	if _, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "z"}); err != nil {
		t.Errorf("after cancelling, a delegate was refused: %v", err)
	}
}

func TestConfigKeepsTheHostOrderWhenRewritten(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	cfg, path, err := loadConfig()
	if err != nil {
		t.Fatal(err)
	}
	cfg.Order = append(cfg.Order, "new")
	cfg.Hosts["new"] = Host{SSH: "new", Claude: "claude", Roots: []string{"~/d"}, Launcher: "tmux", MinFreeMemory: "2G"}
	if err := writeConfig(path, cfg); err != nil {
		t.Fatal(err)
	}
	again, _, err := loadConfig()
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(again.Order, " "); got != "homelab small gpu new" {
		t.Errorf("order = %s", got)
	}
	if again.Hosts["new"].MinFreeMemory != "2G" || again.Hosts["small"].SSH != "me@box" {
		t.Errorf("hosts = %+v", again.Hosts)
	}
}

func TestADelegatedClaudeNeverBypassesPermissions(t *testing.T) {
	args, err := delegateArgs(delegateInput{})
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(args[:8], " "); got != "-p --output-format stream-json --verbose --permission-mode acceptEdits --permission-prompts none" {
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
	f.tool("magi-overview", fakeMagi)
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
	if got := strings.Join(names, " "); got != "cancel_task delegate diff machines run_elsewhere session_elsewhere sessions task_reply task_result task_status" {
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
	for _, dir := range []string{f.home + "/.magi/tasks/..", f.home + "/.magi/tasks/abc", f.home + "/elsewhere/0123abcd", f.home + "/.magi/tasks/0123abcd/.."} {
		if _, err := sh(f.machine(), taskScript, "remove", dir); err == nil {
			t.Errorf("remove accepted %s", dir)
		}
	}
}

const fakeTurnsClaude = `#!/bin/sh
prompt=$(cat)
printf '%s\n' "$@" >> "$HOME/claude-turns"
echo --- >> "$HOME/claude-turns"
case " $* " in
  *" --resume "*) printf two > two.txt ;;
  *) printf one > one.txt ;;
esac
printf '{"type":"result","subtype":"success","num_turns":1,"result":"did: %s"}\n' "$prompt"
`

func TestAFollowUpResumesTheSameClaudeAndMovesTheBranchForward(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", fakeTurnsClaude)
	args, _ := delegateArgs(delegateInput{})
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "first", Claude: args})
	if err != nil {
		t.Fatal(err)
	}
	id := s.Task.ID
	f.waitForTask(id)
	first, err := collectTask(id, false)
	if err != nil {
		t.Fatal(err)
	}
	if !first.Task.Outcome.Kept || !strings.Contains(describeReport(first), "task_reply") {
		t.Fatalf("the copy wasn't kept for follow-ups:\n%s", describeReport(first))
	}
	if _, err := os.Stat(s.Task.Checkout); err != nil {
		t.Fatalf("the copy is gone: %v", err)
	}
	tip := f.git(f.local, "rev-parse", "magi/task-"+id)

	if _, err := replyTask(context.Background(), id, "second"); err != nil {
		t.Fatal(err)
	}
	f.waitForTask(id)
	second, err := collectTask(id, false)
	if err != nil {
		t.Fatal(err)
	}
	o := second.Task.Outcome
	if !strings.HasPrefix(o.Message, "did: second") || second.Task.Turn != 2 {
		t.Errorf("second turn = %+v (turn %d)", o, second.Task.Turn)
	}
	if parent := f.git(f.local, "rev-parse", "magi/task-"+id+"^"); parent != tip {
		t.Errorf("the second turn's commit sits on %s, want the first turn's %s", parent, tip)
	}
	if got := f.git(f.local, "diff", "--name-only", s.Task.Base, "magi/task-"+id); got != "one.txt\ntwo.txt" {
		t.Errorf("the branch changes %q", got)
	}
	if !strings.Contains(o.Diffstat, "two.txt") || strings.Contains(o.Diffstat, "one.txt") {
		t.Errorf("the turn's diffstat = %q", o.Diffstat)
	}
	if !o.Applied || f.read(f.local, "one.txt") != "one" || f.read(f.local, "two.txt") != "two" {
		t.Errorf("both turns' changes aren't here: applied %v, unapplied %q", o.Applied, o.Unapplied)
	}
	if !strings.HasSuffix(o.Log, id+"-2.log") {
		t.Errorf("log = %s", o.Log)
	}
	turns := strings.Split(f.read(f.home, "claude-turns"), "---\n")
	if !strings.Contains(turns[0], "--session-id\n"+s.Task.Session+"\n") || !strings.Contains(turns[1], "--resume\n"+s.Task.Session+"\n") || !strings.Contains(turns[1], "acceptEdits") {
		t.Errorf("claude was run with:\n%s", strings.Join(turns, "---\n"))
	}

	closed, err := collectTask(id, true)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(describeReport(closed), "has been removed") {
		t.Errorf("closing doesn't say so:\n%s", describeReport(closed))
	}
	if _, err := os.Stat(s.Task.Dir); !os.IsNotExist(err) {
		t.Errorf("cancelling didn't remove the kept copy: %v", err)
	}
	if _, err := replyTask(context.Background(), id, "third"); err == nil || !strings.Contains(err.Error(), "is gone") {
		t.Errorf("a reply after closing = %v", err)
	}
}

func TestOnlyADelegatedClaudeTakesFollowUps(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	s, err := startTask(context.Background(), TaskSpec{Kind: kindRun, Host: "server", Dir: f.local, Command: "true"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := replyTask(context.Background(), s.Task.ID, "more"); err == nil || !strings.Contains(err.Error(), "only a delegated Claude") {
		t.Errorf("reply to a run task = %v", err)
	}
	f.tool("claude", "#!/bin/sh\ncat >/dev/null\nsleep 30\n")
	d, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "x"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := replyTask(context.Background(), d.Task.ID, "more"); err == nil || !strings.Contains(err.Error(), "still working") {
		t.Errorf("reply to a running delegate = %v", err)
	}
	collectTask(d.Task.ID, true)
}

func TestANearlyUsedPlanWarnsAndAFullOneRefusesUnlessTheUserInsists(t *testing.T) {
	now := time.Date(2026, 11, 15, 12, 0, 0, 0, time.Local)
	reset := now.Add(90 * time.Minute).Unix()
	plans := []plan{{Agent: "codex", Windows: []planWindow{{Label: "5h", Used: 0.99}}}, {Agent: "claude", Windows: []planWindow{{Label: "5h", Used: 0.93, ResetsAt: reset}, {Label: "weekly", Used: 0.8}}}}
	v := checkPlan(plans, "claude", now)
	if !strings.Contains(v.Refuse, "Claude plan's 5h window is 93% used and resets at 13:30") || !strings.Contains(v.Warn, "weekly window is 80% used") {
		t.Errorf("verdict = %+v", v)
	}
	if v := checkPlan(plans[:1], "claude", now); v.Refuse != "" || v.Warn != "" {
		t.Errorf("another agent's plan counted: %+v", v)
	}
	if _, err := planGate(plans, false); err == nil || !strings.Contains(err.Error(), "despite_plan") {
		t.Errorf("gate = %v", err)
	}
	if text, err := planGate(plans, true); err != nil || !strings.HasPrefix(text, "Started although") {
		t.Errorf("gate despite = %q, %v", text, err)
	}
}

func TestDelegateRefusesWhenMagiSaysThePlanIsNearlyUsedUp(t *testing.T) {
	f := newFixture(t)
	f.config(threeHosts)
	f.tool("magi-overview", strings.Replace(strings.Replace(fakeMagi, `{"machines":[`, `{"plans":[{"agent":"claude","windows":[{"label":"5h","used":0.95,"resets_at":0}]}],"machines":[`, 1),
		`"error":null,"future":"ignored"}`, `"error":null,"future":"ignored","sessions":[{"id":"a","agent":"claude","state":"working","cpu":2.5,"memory":1610612736},{"id":"b","agent":"codex","state":"idle","cpu":0.5,"memory":536870912}]}`, 1))
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
	res, err := session.CallTool(ctx, &mcp.CallToolParams{Name: "delegate", Arguments: map[string]any{"task": "x", "dir": f.local}})
	if err != nil || !res.IsError || !strings.Contains(res.Content[0].(*mcp.TextContent).Text, "5h window is 95% used") {
		t.Fatalf("delegate = %+v, %v", res, err)
	}
	res, err = session.CallTool(ctx, &mcp.CallToolParams{Name: "machines"})
	if err != nil || res.IsError {
		t.Fatalf("machines = %+v, %v", res, err)
	}
	text := res.Content[0].(*mcp.TextContent).Text
	for _, want := range []string{"Claude plan (shared by all machines): 5h window is 95% used", "won't start Claude work until it resets", "its 2 agent sessions use 3.0 cores and 2.0 GB"} {
		if !strings.Contains(text, want) {
			t.Errorf("machines lacks %q:\n%s", want, text)
		}
	}
}

func TestHostsWrittenWithDottedKeysKeepTheirPlace(t *testing.T) {
	f := newFixture(t)
	f.config(`[hosts]
lab.ssh = "lab"
lab.claude = "claude"
lab.roots = ["~/d"]
lab.launcher = "tmux"
`)
	if cfg, _, err := loadConfig(); err != nil || strings.Join(cfg.Order, " ") != "lab" {
		t.Errorf("[hosts] with dotted keys: %v, %v", cfg.Order, err)
	}
	f.config(`hosts.box.ssh = "box"
hosts.box.claude = "claude"
hosts.box.roots = ["~/d"]
hosts.box.launcher = "tmux"

[hosts.lab]
ssh = "lab"
claude = "claude"
roots = ["~/d"]
launcher = "tmux"

[hosts."odd \"name\""]
ssh = "odd"
claude = "claude"
roots = ["~/d"]
launcher = "tmux"
`)
	cfg, path, err := loadConfig()
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(cfg.Order, "|"); got != `box|lab|odd "name"` {
		t.Fatalf("order = %s", got)
	}
	if err := writeConfig(path, cfg); err != nil {
		t.Fatal(err)
	}
	again, _, err := loadConfig()
	if err != nil {
		t.Fatalf("rewritten config doesn't load: %v\n%s", err, f.read(filepath.Dir(path), "config.toml"))
	}
	if got := strings.Join(again.Order, "|"); got != `box|lab|odd "name"` || again.Hosts["lab"].SSH != "lab" {
		t.Errorf("after rewriting: order %s, hosts %+v", got, again.Hosts)
	}
}

func TestMagiSessionsWithoutNumbersYetAreRead(t *testing.T) {
	var data fleetData
	if err := json.Unmarshal([]byte(`{"plans":[],"machines":[{"name":"m","sessions":[{"id":"a","agent":"claude","cpu":null,"memory":null},{"id":"b","agent":"claude","cpu":1.5,"memory":1073741824}]}]}`), &data); err != nil {
		t.Fatal(err)
	}
	if got := data.Machines[0].sessionLoad(); got != "its 2 agent sessions use 1.5 cores and 1.0 GB" {
		t.Errorf("load = %q", got)
	}
}

func TestATaskRecordsWhichClaudeStartedItAndKeepsItsStateCurrent(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	started, err := processStart(context.Background(), os.Getpid())
	if err != nil || time.Since(started) < 0 || time.Since(started) > 10*time.Minute {
		t.Fatalf("this process started %v (%v)", started, err)
	}
	if self, err = callerOf(context.Background(), os.Getpid()); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { self = caller{} })
	if _, err := startElsewhere(context.Background(), "server", TaskSpec{Kind: kindRun, Dir: f.local, Command: "sleep 0.5"}, false); err != nil {
		t.Fatal(err)
	}
	tasks, _ := listTasks()
	id := tasks[0].ID
	record := func() map[string]any {
		var m map[string]any
		dir, _ := tasksDir()
		json.Unmarshal([]byte(f.read(dir, id+".json")), &m)
		return m
	}
	if m := record(); m["caller_pid"] != float64(os.Getpid()) || m["caller_started"] != float64(started.Unix()) || m["state"] != "running" {
		t.Errorf("record = %v", m)
	}
	f.waitForTask(id)
	if m := record(); m["state"] != "done" {
		t.Errorf("after finishing, state = %v", m["state"])
	}
	collectTask(id, false)
	if m := record(); m["state"] != "collected" {
		t.Errorf("after collecting, state = %v", m["state"])
	}
	s, err := startTask(context.Background(), TaskSpec{Kind: kindRun, Host: "server", Dir: f.local, Command: "true"})
	if err != nil {
		t.Fatal(err)
	}
	if s.Task.Caller != 0 || s.Task.CallerAt != 0 {
		t.Errorf("a task started without the tool server has a caller: %+v", s.Task)
	}
}

func TestClosingIdleCopiesIsReportedOnlyWhenCollectingAndNotWhileAnotherMagiHoldsOne(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	idle := Outcome{State: "done", Kept: true, Ended: time.Now().Add(-2 * keepCopies)}
	unreachable := &Task{ID: "0000000a", Kind: kindDelegate, Host: "gone", Outcome: &idle}
	held := &Task{ID: "0000000b", Kind: kindDelegate, Host: "server", Outcome: &idle}
	for _, task := range []*Task{unreachable, held} {
		if err := task.save(); err != nil {
			t.Fatal(err)
		}
	}
	unlock, err := lockTask(held.ID)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	text, err := startElsewhere(context.Background(), "server", TaskSpec{Kind: kindRun, Dir: f.local, Command: "true"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(text, "idle task copies") {
		t.Errorf("starting work reports closing idle copies:\n%s", text)
	}
	note := sweepIdleCopies()
	if !strings.Contains(note, "task 0000000a") || strings.Contains(note, "0000000b") {
		t.Errorf("note = %q", note)
	}
}

const fakeParentClaude = `#!/bin/sh
prompt=$(cat)
echo "parent=$MAGI_PARENT" >> "$HOME/claude-parents"
printf '{"type":"result","subtype":"success","num_turns":1,"result":"did: %s"}\n' "$prompt"
`

func TestADelegatedTaskKeepsItsSendersTabNameAndParentAcrossFollowUps(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	f.tool("claude", fakeParentClaude)
	f.write(filepath.Join(f.home, ".claude", "sessions"), "41.json", `{"pid":41,"sessionId":"9fb6fe92","name":"fix login"}`)
	args, _ := delegateArgs(delegateInput{})
	s, err := startTask(context.Background(), TaskSpec{Kind: kindDelegate, Host: "server", Dir: f.local, Prompt: "first", Claude: args, Caller: caller{PID: 41}})
	if err != nil {
		t.Fatal(err)
	}
	id := s.Task.ID
	label := machineEmoji() + " ← fix login"
	if !strings.Contains(describeStarted(s, "why"), "tab \""+label+"\"") {
		t.Errorf("started:\n%s", describeStarted(s, "why"))
	}
	f.waitForTask(id)
	if _, err := collectTask(id, false); err != nil {
		t.Fatal(err)
	}
	r, err := replyTask(context.Background(), id, "second")
	if err != nil {
		t.Fatal(err)
	}
	if r.Task.tabLabel() != label {
		t.Errorf("the follow-up's tab is %q", r.Task.tabLabel())
	}
	windows, _ := exec.Command("tmux", "list-windows", "-a", "-F", "#{window_name}").Output()
	if strings.Contains(string(windows), " · delegate ") {
		t.Errorf("tmux windows = %q", windows)
	}
	f.waitForTask(id)
	if got := f.read(f.home, "claude-parents"); got != "parent=9fb6fe92\nparent=9fb6fe92\n" {
		t.Errorf("claude ran with %q", got)
	}

	var old Task
	if err := json.Unmarshal([]byte(`{"id":"0000000c","kind":"delegate","name":"api"}`), &old); err != nil {
		t.Fatal(err)
	}
	if old.tabLabel() != "api · delegate 0000000c" || strings.Join(withParent(old.Parent, []string{"sh"}), " ") != "sh" {
		t.Errorf("an older task's tab is %q", old.tabLabel())
	}
}

func TestWaitingOnATaskReturnsOnceItFinishes(t *testing.T) {
	f := newFixture(t)
	f.taskHost("")
	if out, err := exec.Command("tmux", "new-session", "-d", "-s", "acme").CombinedOutput(); err != nil {
		t.Fatalf("tmux: %v %s", err, out)
	}
	s, err := startTask(context.Background(), TaskSpec{Kind: kindRun, Host: "server", Dir: f.local, Command: "sleep 1; echo finished"})
	if err != nil {
		t.Fatal(err)
	}
	r, err := waitTask(s.Task.ID, 100*time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	if r.State.State != "done" || r.State.Exit != "exit 0" || !strings.Contains(r.State.Tail, "finished") {
		t.Fatalf("waited until %+v", r.State)
	}
	if r.Task.Outcome != nil {
		t.Error("waiting collected the task; task_result should")
	}
	if _, err := waitTask("nosuchtask", time.Millisecond); err == nil {
		t.Error("waiting on an unknown task succeeded")
	}
	if text := describeStarted(s, "the first machine with room"); !strings.Contains(text, "magi tasks wait "+s.Task.ID) {
		t.Errorf("the start message doesn't say how to wait:\n%s", text)
	}
}

func TestRemovingACopyThatCantBeFullyDeletedSaysSo(t *testing.T) {
	f := newFixture(t)
	dir := filepath.Join(f.home, ".magi", "tasks", "abcdef12")
	locked := filepath.Join(dir, "checkout", "__pycache__")
	f.write(locked, "x.pyc", "written by root in a container")
	if err := os.Chmod(locked, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Chmod(locked, 0o700) })
	_, err := sh(f.remoteHost(), taskScript, "remove", dir, "")
	if err == nil || !strings.Contains(err.Error(), "couldn't be removed") {
		t.Fatalf("removing reported %v", err)
	}
	os.Chmod(locked, 0o700)
	if _, err := sh(f.remoteHost(), taskScript, "remove", dir, ""); err != nil {
		t.Fatalf("removing a plain copy: %v", err)
	}
	if _, err := os.Stat(dir); !os.IsNotExist(err) {
		t.Fatalf("the copy is still there: %v", err)
	}
}

func TestForgettingATaskRemovesEveryTurnsLog(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	dir, err := tasksDir()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	names := []string{"1a2b3c4d.json", "1a2b3c4d.log", "1a2b3c4d-2.log", "1a2b3c4d-3.log", "9f9f9f9f-2.log"}
	for _, name := range names {
		if err := os.WriteFile(filepath.Join(dir, name), nil, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if err := (&Task{ID: "1a2b3c4d"}).forget(); err != nil {
		t.Fatal(err)
	}
	left, _ := filepath.Glob(filepath.Join(dir, "*"))
	if len(left) != 1 || filepath.Base(left[0]) != "9f9f9f9f-2.log" {
		t.Errorf("left %v", left)
	}
}
