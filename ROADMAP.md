# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code or
  Codex session to another machine and back, and sending work to other
  machines that comes back by itself. That core needs only SSH.
- **One program.** `magi` is a single Go program with SSH as its only
  requirement. It never depends on the overview.
- **The overview is a separate install.** The live view in a Rex pane, the
  agents on every machine and Tailscale live in `magi-overview`, an optional
  install for Rex users. It keeps working but gets no new features for now.
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
  work, and reading machines, sessions and diffs need only SSH. The
  live-control actions (messaging a session, answering its prompts,
  following its work) need `magi-overview`.
- **Follow Rex.** While Rex is in preview, magi supports only its latest
  release. Bump `REX_API` and `rexAPI` with each one.

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

- Moving cloud or Claude Desktop sessions.
- GitHub or pull request workflows.
- Moving or starting sessions by itself.
- New `magi-overview` features.
- Native Windows.
- New ways to delegate work.

## Decided against

- **Live screens through Rex's `data.connect`.** It would need a persistent
  connection and a terminal emulator inside magi, for what the agents' own
  panes already show.
- **Attaching images from magi.** Dropping an image on the session's own Rex
  pane already uploads it to that machine.
