package main

import (
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
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
if [ -n "$MAGI_TEST_REMOTE_CODEX_HOME" ]; then export CODEX_HOME="$MAGI_TEST_REMOTE_CODEX_HOME"; fi
exec "${MAGI_TEST_LOGIN_SHELL:-sh}" -c "$*"
`

type fixture struct {
	t      *testing.T
	home   string
	local  string
	remote string
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
	f.tool("claude", fakeClaude)
	f.tool("codex", fakeCodex)
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

func (f *fixture) remoteHost() *Remote {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	r, err := newRemote(ctx, Host{SSH: "fakehost", Claude: "claude", Codex: "codex"})
	if err != nil {
		f.t.Fatal(err)
	}
	return r
}

func (f *fixture) machine() *Local {
	f.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	f.t.Cleanup(cancel)
	l, err := newLocal(ctx)
	if err != nil {
		f.t.Fatal(err)
	}
	return l
}

func (f *fixture) push(r *Remote, ws Workspace, where Placement) error {
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

const fakeAgents = `if [ "$1 $2" = "agents --json" ]; then
  sep='['
  for f in "$HOME"/.claude/sessions/*.json; do
    pid=${f##*/}
    pid=${pid%.json}
    [ -f "$f" ] && kill -0 "$pid" 2>/dev/null || continue
    printf '%s\n  {\n    "pid": %s' "$sep" "$pid"
    for key in cwd kind startedAt sessionId name status waitingFor; do
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

const fakeClaudeThatStarts = "#!/bin/sh\n" + fakeAgents + `mkdir -p "$HOME/.claude/sessions"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","startedAt":%s000,"bridgeSessionId":"bridge"}' $$ "$2" "$PWD" "$(date +%s)" > "$HOME/.claude/sessions/$$.json"
exec sleep 60
`

const fakeCodex = `#!/bin/sh
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
        case $f in *.jsonl.zst) ;; *) f=${f%%.jsonl*}.jsonl ;; esac
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

const fakeClaudeCLI = `#!/bin/sh
case "$1" in
  --version) echo "2.1.325 (Claude Code)" ;;
  auth) echo '{"loggedIn":true,"authMethod":"claude.ai"}' ;;
  plugin) echo "magi@magi" ;;
esac
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
	f.t.Setenv("TMUX", "")
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
	if err := allowProject(dir, strings.NewReader("y\n"), io.Discard); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) handoffWithProject(id string) Handoff {
	f.t.Helper()
	claude := f.tool("claude", fakeClaudeThatStarts)
	slug, _ := projectSlug(f.local)
	transcript := filepath.Join(f.home, ".claude", "projects", slug, id+".jsonl")
	f.write(filepath.Dir(transcript), id+".jsonl", `{"cwd":"`+f.local+`"}`+"\n")
	return Handoff{
		HostName:       "server",
		Host:           Host{SSH: "fakehost", Claude: claude, Roots: []string{"~/developer"}, LocalRoots: []string{"~/laptop"}},
		SessionID:      id,
		TranscriptPath: transcript,
		Cwd:            f.local,
	}
}
