<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Move a live Claude Code session to another machine and back, with the conversation and your uncommitted work.</b>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#hand-off-and-back">Hand off and back</a> ·
  <a href="#send-work">Send work</a> ·
  <a href="#commands">Commands</a> ·
  <a href="#faq">FAQ</a>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="A Claude Code session on a Mac runs a small server on localhost:3000. The user types hand this off to my homelab. When the turn ends, the session moves to the homelab with the server's uncommitted edit, and the Mac's localhost:3000 now answers from the homelab. Claude there remembers the change and says it is uncommitted. Told to hand this back to the Mac, the session returns." width="820">
</p>

<p align="center">
  <i>A session moves to the homelab and back, and the Mac's localhost:3000 follows it.</i>
</p>

magi is one program. It moves the Claude Code or Codex session you're in to
another machine, with its conversation, branch and uncommitted work, and
brings it back. It also lets Claude send work to your other machines and
get the result back in your working tree. It needs SSH between your
machines, tmux on the machine that receives work, and the Claude Code or
Codex CLI on both.

## Install

On every machine.

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
claude plugin marketplace add felitrejos/magi && claude plugin install magi@magi
```

Then, on each machine that reaches another one over SSH.

```bash
magi init
```

`magi init` asks for the other machine's SSH destination, where `claude`
and your projects live there, and which gitignored files to bring along
(`.env` by default). It writes `~/.config/magi/config.toml` and runs
`magi doctor`, which checks both ends. If Tailscale is installed it lists
the machines online in your tailnet as suggestions. Any SSH that works
without a password prompt will do.

If your laptop takes no incoming SSH, run this on it as well.

```bash
magi watch install
```

It keeps `magi watch` running in the background from login, so sessions on
your other machines can be handed back to the laptop. Run it again after
you add a machine with `magi init`.

Each machine needs git 2.31 or newer, `tar` and Claude Code signed in with
a claude.ai account. The installer puts `magi` in `~/.local/bin`. If you
had `magi-overview`, the installer removes it and its background service.

## Hand off and back

- **Hand off.** Say "hand this off to my homelab" in a Claude Code session.
  When the turn ends, magi copies the conversation, the branch, staged and
  unstaged changes, the gitignored files you listed and Claude's project
  memory. The session continues in a tmux window on the homelab with
  Remote Control on, so it is also in the Claude app. magi prints the
  command that attaches to it. The copy on your machine closes.
- **Hand back.** Say "hand this back to my Mac" there. If the homelab can
  reach the Mac over SSH, that is an ordinary handoff. If it can't, the
  Mac's `magi watch` sees the request over its own SSH connection and
  pulls the session. Until the pull, the session keeps running on the
  homelab.
- **Pull.** Without a watcher, run `magi pull homelab` on the Mac. It
  waits for the session's turn to end, brings it over and continues it in
  that terminal.
- **From outside a session.** `magi move <session-id> <host>` moves a
  session on this machine, Codex sessions too.
- **Claude Desktop.** A session started in Claude Desktop moves the same
  way. Its sidebar entry stays, and if you type into it, magi refuses the
  message and says where the session went. The moved session shows in the
  Desktop sidebar, on claude.ai/code and on your phone through Remote
  Control. A hand back opens it in a tmux window on the laptop under the
  same session id, with Remote Control on, so it stays in the Claude apps.
- **Failures.** A handoff that fails leaves the session running where it
  was. `magi status` lists recent handoffs and sessions waiting to be
  taken back, and stops any copy a failed handoff left behind.

Nothing that is running moves. A project's `.magi.toml` can name a `setup`
command, `services` to start in panes beside the session, and `ports` to
forward, so `localhost:3000` on the machine you left reaches the session's
server over `ssh -L`. It runs only after you approve the file with
`magi allow`.

```toml
setup = "pnpm install"
services = ["pnpm dev"]
ports = [3000]
```

## Send work

<p align="center">
  <img src="assets/background-work.gif" alt="In a Claude Code session on a Mac, the user asks for a background Claude to build a linux/amd64 Docker image on a machine that runs amd64 natively, and meanwhile to add a /stats route on the Mac. Claude proposes the homelab and the user agrees. The Mac session writes /stats while a background Claude on the homelab writes a Dockerfile and builds the image. When it finishes, the Dockerfile and .dockerignore land in the Mac's working tree, and git status shows the uncommitted changes." width="820">
</p>

<p align="center">
  <i>The homelab builds the Docker image while the Mac session keeps working, and the new files land back on the Mac.</i>
</p>

The plugin gives Claude tools to use your other machines.

- **Three kinds of work.** Claude can run a shell command there, give a
  task to a background Claude, or open a second interactive session. Each
  runs in its own copy of your working tree under `~/.magi/tasks`, in a
  tmux window you can watch, and never touches your checkouts there.
- **Claude asks first.** It reads each machine over SSH (system,
  architecture, build tools, load, memory and how many Claude sessions are
  working), proposes one and waits for your yes.
- **Work comes back.** Claude waits with `magi tasks wait <id>` in the
  background. A background Claude's changes are applied to your working
  tree and kept on the branch `magi/task-<id>`. If they clash with your
  edits, nothing is overwritten and they stay on the branch.
- **Limits per machine.** Under a host in `config.toml` you can set
  `max_agents`, `min_free_memory` (such as `"4G"`) and `quiet_hours` (such
  as `"22:00-07:00"`). magi starts nothing new on a machine past them.

`magi mcp` serves the same tools to any MCP client. For Claude Desktop, add
this to `claude_desktop_config.json`.

```json
{ "mcpServers": { "magi": { "command": "/Users/you/.local/bin/magi", "args": ["mcp"] } } }
```

| Tool | What it does |
|---|---|
| `machines` | Lists your machines with their system, load and room |
| `sessions` | Lists Claude Code and Codex sessions on every machine |
| `diff` | Shows a session's uncommitted changes |
| `run_elsewhere` | Runs a shell command on another machine |
| `delegate` | Gives a task to a background Claude there |
| `session_elsewhere` | Opens a new interactive session there |
| `task_status`, `task_result`, `task_reply`, `cancel_task` | Follow, collect, continue or stop that work |

## Commands

```text
magi init                          set up this machine and a machine to hand off to
magi doctor [host...]              check that everything a handoff needs is in place
magi pull <host> [session-id]      bring a session back from <host> and continue it here
magi move <session-id> <host> [--json]
                                   move a session on this machine to <host>
magi status                        recent handoffs, sessions waiting to be taken back,
                                   parked checkouts and leftover copies
magi watch                         take back sessions other machines hand back here
magi watch install | uninstall     run magi watch in the background from login
magi unpark [dir]                  stop treating a checkout as parked
magi allow [dir]                   let the project's .magi.toml run after handoffs
magi forwards                      ports this machine forwards to sessions that moved
magi forwards close <session-id>   stop forwarding a moved session's ports
magi tasks                         work sent to other machines, and its state
magi tasks wait | show | cancel <id>
magi tasks clean                   collect finished tasks and drop old copies
```

## FAQ

**How do I move a Codex session?** With `magi move` or `magi pull`. It
moves when its turn ends and reopens with `codex resume`. If another Codex
is open in the same folder, quit it first, because magi can't tell which
session it shows. The other machine needs Codex installed, and `magi init`
asks for its path.

**Where is my session after a handoff?** In a tmux window on the other
machine, and in the Claude app through Remote Control. The copy you left
refuses new messages and tells you where the session went.

**What if Claude Code changes its files?** magi reads session files that
aren't a documented format, so it only moves a session with Claude Code
and Codex versions its tests passed with. If either machine runs a newer
one, magi refuses the move before it copies anything and names the
version. The tests run every night against the newest releases, so this
usually clears within a day, without a magi update. `magi doctor` shows it
for every machine. To get the list of tested versions, magi fetches one
small file from this repository on GitHub, at most once an hour. That is
the only web request it makes.

What's next is in [ROADMAP.md](ROADMAP.md). What magi reads, sends and
changes is in [SECURITY.md](SECURITY.md). Licensed [MIT](LICENSE).
