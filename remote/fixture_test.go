package main

import (
	"cmp"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/felitrejos/magi/remote/internal/sh"
	"github.com/modelcontextprotocol/go-sdk/mcp"
)

const fakeSSH = `#!/bin/sh
if [ "$1" = -G ]; then
  shift
  if [ "$1" = -- ]; then shift; fi
  name=$(awk -v h="$1" '$1 == h {print $2}' "$HOME/.fake-ssh-hosts" 2>/dev/null)
  echo "hostname ${name:-$1}"
  exit 0
fi
host=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) shift 2 ;;
    --) shift; break ;;
    *) if [ -z "$host" ]; then host=$1; shift; else break; fi ;;
  esac
done
if [ -s "$HOME/ssh-down" ]; then
  cat "$HOME/ssh-down" >&2
  exit 255
fi
if [ -e "$HOME/ssh-down" ]; then
  echo "ssh: connect to host $host port 22: Network is unreachable" >&2
  exit 255
fi
if [ -n "$MAGI_TEST_REMOTE_HOME" ]; then export HOME="$MAGI_TEST_REMOTE_HOME"; fi
if [ -n "$MAGI_TEST_REMOTE_CODEX_HOME" ]; then export CODEX_HOME="$MAGI_TEST_REMOTE_CODEX_HOME"; fi
exec "${MAGI_TEST_LOGIN_SHELL:-sh}" -c "$*"
`

type fixture struct {
	t      *testing.T
	home   string
	local  string
	remote string
	answer string
	asked  []string
}

func newFixture(t *testing.T) *fixture {
	t.Helper()
	home, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	bin := filepath.Join(home, "bin")
	f := &fixture{t: t, home: home, local: filepath.Join(home, "laptop", "acme", "api"), remote: filepath.Join(home, "developer", "acme", "api")}
	f.write(bin, "ssh", fakeSSH)
	if err := os.Chmod(filepath.Join(bin, "ssh"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("HOME", home)
	t.Setenv("PATH", bin+":"+os.Getenv("PATH"))
	t.Setenv("TMUX", "")
	t.Setenv("TMUX_PANE", "")
	builtin, err := parseTested(builtinTested)
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("MAGI_FAKE_CLAUDE_VERSION", builtin.Claude.Max)
	t.Setenv("MAGI_FAKE_CODEX_VERSION", builtin.Codex.Max)
	url := testedURL
	testedURL = ""
	t.Cleanup(func() { testedURL = url })
	f.tool("claude", fakeClaude)
	f.tool("codex", fakeCodex)
	f.tool("launchctl", "#!/bin/sh\necho \"$*\" >> \"$HOME/service-calls\"\n")
	f.tool("systemctl", "#!/bin/sh\necho \"$*\" >> \"$HOME/service-calls\"\n")
	t.Setenv("GIT_CONFIG_GLOBAL", "/dev/null")
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("XDG_CONFIG_HOME", "")
	t.Setenv("GIT_AUTHOR_NAME", "t")
	t.Setenv("GIT_AUTHOR_EMAIL", "t@t")
	t.Setenv("GIT_COMMITTER_NAME", "t")
	t.Setenv("GIT_COMMITTER_EMAIL", "t@t")

	f.git(home, "init", "-q", "-b", "main", f.local)
	f.git(f.local, "remote", "add", "origin", "git@github.com:acme/api.git")
	f.write(f.local, ".gitignore", ".env*\nnode_modules/\n")
	f.write(f.local, "kept.txt", "v1")
	f.write(f.local, "edited.txt", "v1")
	f.write(f.local, "gone.txt", "v1")
	f.write(f.local, "staged-gone.txt", "v1")
	f.git(f.local, "add", ".")
	f.git(f.local, "commit", "-q", "-m", "init")
	f.git(home, "clone", "-q", f.local, f.remote)
	f.git(f.remote, "remote", "set-url", "origin", "https://github.com/acme/api")
	return f
}

func (f *fixture) git(dir string, args ...string) string {
	f.t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		f.t.Fatalf("git %v: %v\n%s", args, err, out)
	}
	return strings.TrimSpace(string(out))
}

func (f *fixture) write(dir, name, body string) {
	f.t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) read(dir, name string) string {
	f.t.Helper()
	data, err := os.ReadFile(filepath.Join(dir, name))
	if err != nil {
		return "<missing>"
	}
	return string(data)
}

func (f *fixture) remoteHost() *sh.Remote {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	r, err := newRemote(ctx, Host{SSH: "fakehost", Claude: "claude", Codex: "codex"})
	if err != nil {
		f.t.Fatal(err)
	}
	return r
}

func (f *fixture) machine() *sh.Local {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	l, err := sh.NewLocal(ctx)
	if err != nil {
		f.t.Fatal(err)
	}
	return l
}

func (f *fixture) push(r *sh.Remote, ws Workspace, where Placement) error {
	l := f.machine()
	return syncProject(l, r, ws, where, nil, func() error { return pushGit(l, r, ws, where.Root) })
}

func (f *fixture) workspace(dir string) Workspace {
	f.t.Helper()
	ws, err := readWorkspace(f.machine(), dir, []string{".env", ".env.*"}, []string{"node_modules"})
	if err != nil {
		f.t.Fatal(err)
	}
	return ws
}

func (f *fixture) messUp(dir string) {
	f.t.Helper()
	f.write(dir, "committed.txt", "from a commit")
	f.git(dir, "add", "committed.txt")
	f.git(dir, "commit", "-q", "-m", "work")
	f.write(dir, "edited.txt", "staged")
	f.git(dir, "add", "edited.txt")
	f.write(dir, "edited.txt", "staged then edited")
	f.write(dir, "new-staged.txt", "new")
	f.git(dir, "add", "new-staged.txt")
	f.write(dir, "dir with space/untracked.bin", "\x00\x01binary")
	if err := os.Remove(filepath.Join(dir, "gone.txt")); err != nil {
		f.t.Fatal(err)
	}
	f.git(dir, "rm", "-q", "staged-gone.txt")
	f.write(dir, ".env", "SECRET=1")
	f.write(dir, "node_modules/pkg/.env", "not ours")
}

func waitForFile(t *testing.T, path string, within time.Duration) bool {
	t.Helper()
	deadline := time.Now().Add(within)
	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			return true
		}
		time.Sleep(100 * time.Millisecond)
	}
	return false
}

func startFakeClaude(t *testing.T, seconds string) *exec.Cmd {
	t.Helper()
	cmd := exec.Command("sleep", seconds)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	go cmd.Wait()
	t.Cleanup(func() { cmd.Process.Kill() })
	return cmd
}

const fakeAgents = `if [ "$1" = --version ]; then echo "$MAGI_FAKE_CLAUDE_VERSION (Claude Code)"; exit; fi
if [ "$1 $2" = "agents --json" ]; then
  sep='['
  for f in "$HOME"/.claude/sessions/*.json; do
    pid=${f##*/}
    pid=${pid%.json}
    [ -f "$f" ] && kill -0 "$pid" 2>/dev/null || continue
    printf '%s\n  {\n    "pid": %s' "$sep" "$pid"
    for key in cwd startedAt sessionId name status waitingFor; do
      value=$(sed -nE 's/.*"'"$key"'":("[^"]*"|[0-9]+).*/\1/p' "$f")
      if [ -n "$value" ]; then printf ',\n    "%s": %s' "$key" "$value"; fi
    done
    printf '\n  }'
    sep=,
  done
  if [ "$sep" = '[' ]; then echo '[]'; else printf '\n]\n'; fi
  exit
fi
`

const fakeClaude = "#!/bin/sh\n" + fakeAgents

const fakeStart = `mkdir -p "$HOME/.claude/sessions"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","startedAt":%s000,"bridgeSessionId":"bridge"}' $$ "$2" "$PWD" "$(date +%s)" > "$HOME/.claude/sessions/$$.json"
`

const fakeClaudeThatStarts = "#!/bin/sh\n" + fakeAgents + fakeStart + "exec sleep 60\n"

const fakeClaudeThatAnswers = "#!/bin/sh\n" + fakeAgents + fakeStart + `sleep 1
printf '%s\n' '{"type":"user","message":{"content":"[magi] This session was just handed off"}}' '{"type":"assistant","message":{"content":"on server"}}' >> "$HOME/.claude/projects/$(printf '%s' "$PWD" | sed 's/[^A-Za-z0-9]/-/g')/$2.jsonl"
exec sleep 60
`

const fakeCodex = `#!/bin/sh
if [ "$1" = --version ]; then echo "codex-cli $MAGI_FAKE_CODEX_VERSION"; exit; fi
codex_home=${CODEX_HOME:-$HOME/.codex}
config=$codex_home/config.toml
if [ "$1" = app-server ]; then
  mkdir -p "$codex_home"
  while read -r line; do
    printf '%s\n' "$line" >> "$codex_home/app-server-calls"
    id=${line#'{"id":'}
    id=${id%%,*}
    case $line in
      *'"method":"initialize"'*) printf '{"id":0,"result":{"codexHome":"%s"}}\n' "$codex_home" ;;
      *'"method":"thread/read"'*)
        thread=${line##*'"threadId":"'}
        thread=${thread%%'"'*}
        f=$(find "$codex_home/sessions" -name "rollout-*-$thread.jsonl*" 2>/dev/null | head -n 1)
        if [ -z "$f" ]; then
          printf '{"id":%s,"error":{"code":-32600,"message":"thread not loaded: %s"}}\n' "$id" "$thread"
          continue
        fi
        cwd=$(grep -E '"type":"(session_meta|turn_context)"' "$f" | tail -n 1 | sed 's/.*"cwd":"\([^"]*\)".*/\1/')
        f=${f%%.jsonl*}.jsonl
        printf '{"method":"thread/status/changed","params":{}}\n{"id":%s,"result":{"thread":{"id":"%s","name":null,"path":"%s","cwd":"%s","status":{"type":"notLoaded"}}}}\n' "$id" "$thread" "$f" "$cwd"
        ;;
      *'"method":"config/read"'*)
        projects=$(awk '/^\[/ { key = "" } /^\[projects\./ { key = substr($0, 11, length($0) - 11) } /^trust_level/ && key != "" { printf "%s%s:{\"trust_level\":%s}", sep, key, $3; sep = "," }' "$config" 2>/dev/null)
        printf '{"id":%s,"result":{"config":{"projects":{%s}},"layers":[{"name":{"type":"user"},"version":"%s"}]}}\n' "$id" "$projects" "$(cksum 2>/dev/null < "$config")"
        ;;
      *'"method":"config/value/write"'*)
        expected=${line##*'"expectedVersion":"'}
        expected=${expected%%'"'*}
        if [ "$expected" != "$(cksum 2>/dev/null < "$config")" ]; then
          printf '{"id":%s,"error":{"code":-32600,"data":{"config_write_error_code":"configVersionConflict"},"message":"Configuration was modified since last read. Fetch latest version and retry."}}\n' "$id"
          continue
        fi
        key=${line#*'"keyPath":"projects.'}
        key=$(printf '%s' "${key%%.trust_level*}" | sed 's/\\"/"/g')
        printf '\n[projects.%s]\ntrust_level = "trusted"\n' "$key" >> "$config"
        printf '{"id":%s,"result":{"status":"ok"}}\n' "$id"
        ;;
    esac
  done
  exit
fi
printf '%s\n' "$@" > "$HOME/codex-args"
while [ $# -gt 0 ] && [ "$1" != resume ]; do shift; done
f=$(find "$codex_home/sessions" -name "rollout-*-$2.jsonl")
printf '%s\n' '{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}' '{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete"}}' >> "$f"
sleep 60
`

func (f *fixture) tool(name, body string) string {
	f.t.Helper()
	path := filepath.Join(f.home, "bin", name)
	f.write(filepath.Dir(path), name, body)
	if err := os.Chmod(path, 0o755); err != nil {
		f.t.Fatal(err)
	}
	return path
}

func (f *fixture) config(body string) {
	f.t.Helper()
	f.write(filepath.Join(f.home, ".config", "magi"), "config.toml", body)
}

func (f *fixture) isolatedTmux() {
	f.t.Helper()
	if _, err := exec.LookPath("tmux"); err != nil {
		f.t.Skip("tmux isn't installed")
	}
	sockets, err := os.MkdirTemp("", "tm")
	if err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(func() { os.RemoveAll(sockets) })
	f.t.Setenv("TMUX_TMPDIR", sockets)
	f.t.Cleanup(func() {
		out, _ := exec.Command("tmux", "list-panes", "-a", "-F", "#{pane_pid}").Output()
		exec.Command("tmux", "kill-server").Run()
		deadline := time.Now().Add(5 * time.Second)
		for _, field := range strings.Fields(string(out)) {
			pid, _ := strconv.Atoi(field)
			for pid > 0 && syscall.Kill(pid, 0) == nil && time.Now().Before(deadline) {
				time.Sleep(50 * time.Millisecond)
			}
		}
	})
}

func (f *fixture) allow(dir string) {
	f.t.Helper()
	p, err := readProject(dir)
	if err != nil {
		f.t.Fatal(err)
	}
	if text, failed := f.call("allow_project", map[string]any{"dir": dir, "sha256": p.Hash}); failed {
		f.t.Fatal(text)
	}
}

func (f *fixture) call(tool string, args map[string]any) (string, bool) {
	f.t.Helper()
	serverSide, clientSide := mcp.NewInMemoryTransports()
	ctx := context.Background()
	if _, err := newMCPServer().Connect(ctx, serverSide, nil); err != nil {
		f.t.Fatal(err)
	}
	ask := func(_ context.Context, req *mcp.ElicitRequest) (*mcp.ElicitResult, error) {
		f.asked = append(f.asked, req.Params.Message)
		return &mcp.ElicitResult{Action: cmp.Or(f.answer, "accept")}, nil
	}
	session, err := mcp.NewClient(&mcp.Implementation{Name: "test", Version: "1"}, &mcp.ClientOptions{ElicitationHandler: ask}).Connect(ctx, clientSide, nil)
	if err != nil {
		f.t.Fatal(err)
	}
	defer session.Close()
	res, err := session.CallTool(ctx, &mcp.CallToolParams{Name: tool, Arguments: args})
	if err != nil {
		f.t.Fatal(err)
	}
	return res.Content[0].(*mcp.TextContent).Text, res.IsError
}

func (f *fixture) serverHome() string {
	f.t.Helper()
	server := filepath.Join(f.home, "server")
	f.t.Setenv("MAGI_TEST_REMOTE_HOME", server)
	f.remote = filepath.Join(server, "developer", "acme", "api")
	return server
}

func (f *fixture) handoffToServer(id string) Handoff {
	f.t.Helper()
	f.isolatedTmux()
	claude := f.tool("claude", fakeClaudeThatStarts)
	slug, err := projectSlug(f.local)
	if err != nil {
		f.t.Fatal(err)
	}
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	return Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, Secrets: []string{".env"}, LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
		Retire:         func() (int, error) { return 0, nil },
	}
}

func runProgram(t *testing.T, name string, args ...string) <-chan error {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, []byte("#!/bin/sh\nsleep 30\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(path, args...)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait(); close(done) }()
	t.Cleanup(func() { cmd.Process.Kill(); <-done })
	return done
}
