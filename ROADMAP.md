# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code or
  Codex session to another machine and back, and sending work to other
  machines that comes back by itself. That core needs only SSH.
- **One program.** `magi` is a single Go program with SSH as its only
  requirement. It never depends on the overview.
- **A menu bar replaces the overview.** The overview's terminal view in
  Rex, its agents on every machine and Tailscale are being replaced by a Mac
  menu bar app built on `magi` over SSH. Until then `magi-overview` keeps
  working and gets no new features.
- **Reliable before new.** A handoff should be boring. Work on making moves
  safe and measured comes before anything else.
- **tmux as good as Rex.** Everything the core does in a Rex tab it does in
  a tmux window too.
- **Leave it to the tools.** What Claude Code, Codex, Rex or tmux already do
  stays there.
- **Suggest, don't act.** magi suggests moves and machines but never moves
  or starts anything by itself. It only cleans up after itself. It closes a
  port forward left pointing at a machine its session has moved on from,
  and `magi status` and the next handoff stop a copy that a failed handoff
  left running.
- **A layer for other tools.** The CLI and `magi mcp` give scripts, other
  orchestrators and Claude itself magi's actions. Moving sessions, sending
  work, reading machines, sessions and diffs, waiting on a session and
  moving another session all go through `magi` over SSH. No tool types into
  another live session. The session's own app (Desktop, the Claude app
  through Remote Control, or its terminal) is where you answer it.
- **Follow Rex.** Rex is optional. magi uses it when it's there, for tabs
  and port forwards, and falls back to tmux and its own SSH tunnels. While
  Rex is in preview, magi supports only its latest release. Bump `REX_API`
  and `rexAPI` with each one.

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
- **Hand back with SSH only.** Today, handing a session back to a laptop
  that doesn't accept incoming SSH needs `magi-overview`. In 1.0 it works
  with only the laptop's outgoing SSH.
- **Claude Desktop parity.** Everything works the same from a Claude Desktop
  session as from the CLI. Handing off from Desktop works, and so does
  handing back into it. The session reopens in the same Desktop entry with
  the whole conversation, and the note telling it where it now is arrives
  with the next message. Two cases are left. One is the same hand back when
  the Mac accepts SSH and the other machine hands the session back itself
  instead of the Mac pulling it. The other is Desktop sessions that run in
  a git worktree, since Desktop removes the worktree when it archives the
  session.
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

## After 1.0: a menu bar instead of the overview

A quick glance at every machine and every session, in the Mac menu bar,
read-only.

- **Session state from hooks.** The plugin's hooks record each session's
  state on its machine, working, waiting on you or idle, and `sessions`
  reports it. Asking "does anything need me?" is then one `sessions` call.
- **Two more MCP tools in Go.** `wait` waits until another session finishes,
  such as one handed off to another machine. `move_session` moves a session
  other than the one you're in, using `magi move`. The overview's other
  tools are dropped. `alerts` goes because its machine and plan parts are
  already in `machines` and its sessions part becomes the state above.
  `recent_work` and `subagents` go, and so does `start_session`, which the
  plugin's `session_elsewhere` covers. The three that type into a live
  session, `send_message`, `answer_permission` and `choose_option`, go too.
- **`magi watch`.** One SSH connection per machine, started by the Mac,
  streams what changes there. That is machine stats, sessions and their
  state, and the CPU and memory each session uses, counting what it started.
  It reacts to hooks and file changes rather than polling. Idle cost is
  measured on the Mac and on Linux against a budget of 0.5% CPU and 30 MB.
  Only if Go misses it does the watcher move to Rust.
- **The menu bar app.** SwiftUI, Mac only, open source. It shows each
  machine's CPU, memory, load, disk and whether it answers. For each session
  it shows the project, the state, how long it's been in it, and its CPU and
  memory. It also shows where sessions moved, background tasks, and plan
  limits, which come from an undocumented endpoint and may break. A badge
  shows when a session is waiting on you. The only click opens a session
  where it lives, in Desktop, its Rex tab or its tmux window. It doesn't
  answer prompts or type into sessions. It reconnects quietly after sleep or
  a network change and shows a machine that doesn't answer instead of hiding
  it.
- **Shipping it.** Not notarized. Installed from a Homebrew tap of the
  project's own, with the README saying how to open it the first time
  (right-click and Open, or `xattr -dr com.apple.quarantine`). Updates
  through that tap.
- **Delete the overview.** Once the menu bar covers what the overview did,
  remove `magi-overview`. That means the Rust crate, its terminal view, its
  agents and the Tailscale code, and `sh -s overview` from the installer.
  Update the README, SECURITY.md and PRIVACY.md.

## Decided against

- **Live screens through Rex's `data.connect`.** It would need a persistent
  connection and a terminal emulator inside magi, for what the agents' own
  panes already show.
- **Attaching images from magi.** Dropping an image on the session's own Rex
  pane already uploads it to that machine.
- **Actions in the menu bar.** Answering prompts or typing into a session
  from the menu bar would duplicate the session's own app and need hooks
  that wait for an answer. Opening the session is enough.
- **Tools that type into other sessions.** Remote Control and Desktop
  already let you answer a session yourself.
