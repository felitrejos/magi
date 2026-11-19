# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code or
  Codex session to another machine and back, and sending work to other
  machines that comes back by itself.
- **One program.** `magi` is a single Go program. It needs SSH, tmux on
  the machine that receives work, and the Claude Code or Codex CLI.
- **Reliable before new.** A handoff should be boring. Work on making moves
  safe and measured comes before anything else.
- **Leave it to the tools.** What Claude Code, Codex, SSH or tmux already
  do stays there.
- **Suggest, don't act.** magi suggests machines but never moves or starts
  anything by itself. The one exception is `magi watch`, which takes back a
  session you asked to hand back. Beyond that it only cleans up after
  itself. `magi status` and the next handoff stop a copy that a failed
  handoff left running.
- **A layer for other tools.** The CLI and `magi mcp` give scripts, other
  orchestrators and Claude itself magi's actions. No tool types into
  another live session. The session's own app (Desktop, the Claude app
  through Remote Control, or its terminal) is where you answer it.

## 1.0

1.0 is for a developer with two or more machines who wants one live coding
session, Claude Code or Codex, to follow them from one machine to the next.
The session arrives with its uncommitted work and its context, and there is
exactly one live copy at any time. magi is not a cloud workspace.

- **Keep up with Claude Code and Codex.** The core leans on their
  undocumented session files and hooks, and that is the biggest risk to the
  project. CI runs the handoff tests against the latest Claude Code and
  Codex releases. `magi doctor` refuses a move when it finds a version it
  hasn't been tested with.
- **A guided first run.** `magi try` takes a new user from nothing to a
  verified round trip between two machines. It checks the SSH key, that
  Claude is signed in on the other machine and that the plugin is installed
  there, then moves a test session there and back. A failure says exactly
  what to fix.
- **Claude Desktop parity.** Everything works the same from a Claude Desktop
  session as from the CLI. Handing off from Desktop works, and so does
  handing back into it when `magi watch` takes the session back, with the
  note telling it where it now is delivered as it reopens. Three cases
  are left. One is the hand back when the Mac accepts SSH and the other
  machine hands the session back itself. Another is `magi pull` typed in a
  terminal, which continues the session in that terminal. The last is
  Desktop sessions that run in a git worktree, since Desktop removes the
  worktree when it archives the session.
- **Recovery anyone can follow.** When a move fails because SSH drops, the
  machine sleeps or the hook is killed, `magi status` says which copy is the
  live one, cleans up the other and prints the exact command to continue.
  Crash and network-drop tests run in CI, and their results are published
  alongside the round-trip numbers.
- **Show what gets copied.** Before a move, magi prints a short list of what
  goes across, with files outside git, the secrets files the project listed,
  ports and the services in the project's `.magi.toml`. Secrets show by
  name, never by value. Defaults are conservative and SSH host key checks
  stay on.
- **A support matrix.** Mac to Linux and Linux to Linux are tested on every
  release. WSL is either tested end to end or marked unsupported, in the
  docs and in `magi init`. A table shows what works for Claude Code and for
  Codex, since Codex has no end-of-turn hook and moves differently.
- **Docs a stranger can use.** A five-minute tutorial that goes from one
  machine to a second one, a troubleshooting page, a short versioning policy
  that says what 1.x promises not to break, and a changelog.

### Not in 1.0

- Moving cloud sessions.
- GitHub or pull request workflows.
- Moving or starting sessions by itself.
- The menu bar app, which comes right after.
- Native Windows.
- New ways to delegate work.

## After 1.0: a menu bar

A quick glance at every machine and every session, in the Mac menu bar,
read-only.

- **Session state from hooks.** The plugin's hooks record each session's
  state on its machine, working, waiting on you or idle, and `sessions`
  reports it. Asking "does anything need me?" is then one `sessions` call.
- **Two more MCP tools.** `wait` waits until another session finishes,
  such as one handed off to another machine. `move_session` moves a session
  other than the one you're in, using `magi move`.
- **`magi watch` streams changes.** Today the watcher only takes sessions
  back. Over the same SSH connection per machine it will also stream what
  changes there. That is machine stats, sessions and their state, and the
  CPU and memory each session uses, counting what it started. It reacts to
  hooks and file changes rather than polling. Idle cost is measured on the
  Mac and on Linux against a budget of 0.5% CPU and 30 MB.
- **The menu bar app.** SwiftUI, Mac only, open source. It shows each
  machine's CPU, memory, load, disk and whether it answers. For each session
  it shows the project, the state, how long it's been in it, and its CPU and
  memory. It also shows where sessions moved and background tasks. A badge
  shows when a session is waiting on you. The only click opens a session
  where it lives, in Desktop or its tmux window. It doesn't answer prompts
  or type into sessions. It reconnects quietly after sleep or a network
  change and shows a machine that doesn't answer instead of hiding it.
- **Shipping it.** Not notarized. Installed from a Homebrew tap of the
  project's own, with the README saying how to open it the first time
  (right-click and Open, or `xattr -dr com.apple.quarantine`). Updates
  through that tap.

## Decided against

- **Actions in the menu bar.** Answering prompts or typing into a session
  from the menu bar would duplicate the session's own app and need hooks
  that wait for an answer. Opening the session is enough.
- **Tools that type into other sessions.** Remote Control and Desktop
  already let you answer a session yourself.
