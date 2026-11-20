# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code
  session to another machine and back, and sending work to other machines
  that comes back by itself.
- **One program.** `magi` is a single Go program. It needs SSH, tmux and
  the Claude Code CLI.
- **Ask, don't type.** Nobody types magi commands. Installing the plugin
  is the whole install, and everything after that is something you ask
  Claude for.
- **Reliable before new.** A handoff should be boring. Work on making moves
  safe and measured comes before anything else.
- **Leave it to the tools.** What Claude Code, SSH or tmux already do
  stays there.
- **Suggest, don't act.** magi suggests machines but never moves or starts
  anything by itself. The one exception is `magi watch`, which takes back a
  session you asked to hand back. Beyond that it only cleans up after
  itself, which includes settling a move that was cut off.
- **A layer for other tools.** `magi mcp` gives Claude and any other MCP
  client magi's actions. No tool types into another live session. You
  answer a session in its own app, which is the Claude app through Remote
  Control or its terminal.

## 1.0

1.0 is for a developer with two or more machines who wants one live Claude
Code session to follow them from one machine to the next. The session
arrives with its uncommitted work and its context, and there is exactly
one live copy at any time. A read-only menu bar app on the Mac shows every
machine and every session at a glance. magi is not a cloud workspace.

What is left for 1.0 is the menu bar app and shipping it, plus the open
points listed under the items below.

### Left to do

- **The menu bar app.** Not built yet. SwiftUI, Mac only, open source. It
  reads the files `magi watch` writes in `~/.magi/live`. It shows each
  machine's load against its cores, memory, disk and whether it answers,
  which includes a machine not heard from for about 40 seconds. For each
  session it shows the project, the state, how long it's been in it, and
  its CPU and memory. It also shows where sessions moved and background
  tasks, which the live files don't hold. A badge shows when a session is
  waiting on you. The only click opens a session where it lives. It
  doesn't answer prompts or type into sessions. It reconnects quietly
  after sleep or a network change and shows a machine that doesn't answer
  instead of hiding it.
- **Shipping the menu bar app.** Not notarized. Installed from a Homebrew
  tap of the project's own, with the README saying how to open it the
  first time (right-click and Open, or `xattr -dr com.apple.quarantine`).
  Updates through that tap.

### Done, with open points

- **Agent native.** Done in the code. The typed commands and the install
  script are gone. You add the plugin in Claude Code, and its launcher
  downloads the matching release binary and checks its SHA-256.
  `setup_machine` checks SSH, installs the plugin on the other machine,
  starts the watcher, checks both machines and runs a throwaway round
  trip. It ran end to end between a Mac and a Linux server. Still open
  are these.
  - The plugin's version, 0.10.0, has no published release. Until one
    exists the launcher has nothing to download, and only a local build
    works.
  - Releases aren't signed. The launcher's checksum comes from the same
    release as the binary.
- **Keep up with Claude Code.** The core leans on Claude Code's
  undocumented session files and hooks, and that is the biggest risk to
  the project. `remote/tested.json` holds the oldest supported version,
  the newest one the contract tests passed with and a list of versions
  that fail. A nightly GitHub Actions workflow runs the contract tests on
  Ubuntu and macOS runners, raises the newest tested version when both
  pass, and opens an issue that names the failed job otherwise. A move in
  either direction and the checks in `setup_machine` refuse a version
  outside the list, and pick up a new list within an hour without a magi
  update. Still open are these.
  - The nightly depends on one Claude account, through the repository
    secret `CLAUDE_CODE_OAUTH_TOKEN`.
  - The list of failing versions is edited by hand.
- **A support matrix.** [docs/support.md](docs/support.md) has the table,
  what tests each row, and the measured numbers. WSL is marked unsupported
  there, and `setup_machine` refuses it. Still open are these.
  - No CI job moves a session between two real machines. The real round
    trip is a manual check from a Mac to a Linux server.
  - Linux to Linux and Mac to Mac have never run between two real
    machines.
- **`magi watch` streams changes.** Over the one SSH connection per
  machine it keeps for hand backs, the watcher streams what changes
  there, and runs the same loop for this machine. It writes the latest to
  `~/.magi/live/<machine>.json`. [SECURITY.md](SECURITY.md) lists what
  the loop reads and what the file holds. Idle cost is measured on Linux
  and on a Mac, inside the budget of 0.5% of one core and 30 MB. Stopping
  the watcher marks every machine as not answering. A new loop stops
  orphaned old loops for its machine. Still open are these.
  - On macOS, per-session CPU misses a child that started and exited
    between two samples. macOS doesn't expose that to other processes.
    Linux counts it.
  - When the watcher is killed outright, the live files keep their last
    content. A reader should still treat a `heard` time older than about
    40 seconds as the machine not answering.

### Done

- **Recovery anyone can follow.** When a move is cut off because SSH
  drops, the machine sleeps or the hook is killed, magi works out which
  copy is the live one and cleans up the other. It does that from a
  journal per move, when a Claude Code session starts, once a minute from
  the watcher and before the next move of that session. It never deletes
  a folder, and it stops nothing when it can't tell which copy is live.
  Tests kill the hook at each step and take the network down. The results
  of real crash and network-drop runs are in
  [docs/support.md](docs/support.md).
- **Show what gets copied.** Before a move, magi lists what goes across,
  with its size, for a handoff, for `move_session` in both directions and
  for `magi watch`. Claude asks you first when secret files, untracked
  files, or 50 MB or more would go. Untracked files whose names are in
  `exclude` stay behind and are listed as left out. The default `exclude`
  and `secrets` apply when the config has none. magi never turns SSH host
  key checks off.
- **One pipeline for moves.** A push and a pull run the same steps. Both
  carry folder trust, run an approved project's setup in panes, and give
  the Remote Control link.
- **Docs a stranger can use.** A [tutorial](docs/tutorial.md), a
  [troubleshooting page](docs/troubleshooting.md), a
  [versioning policy](docs/versioning.md) and a
  [changelog](CHANGELOG.md).
- **Session state.** `sessions` opens with every session waiting on you
  across your machines, and any machine it couldn't check. The state,
  busy, waiting or idle, comes from Claude Code itself through
  `claude agents --json`, so magi adds no hooks for it.
- **Wait for a session, move another one.** `magi wait` returns once a
  session on any machine isn't busy and says whether it's idle, waiting on
  you or ended. `move_session` moves a session other than the one you're
  in, in either direction, after your yes. It doesn't move a session that
  is busy or waiting on you. `wait` is a command Claude runs in a
  background shell and not a tool, because a tool call that blocks for
  minutes runs into MCP time limits.
- **Claude Code only.** Codex support is removed. magi moves Claude Code
  sessions and nothing else.

### Not in 1.0

- Moving cloud sessions.
- GitHub or pull request workflows.
- Moving or starting sessions by itself.
- Native Windows and WSL.
- New ways to delegate work.

## Decided against

- **Actions in the menu bar.** Answering prompts or typing into a session
  from the menu bar would duplicate the session's own app and need hooks
  that wait for an answer. Opening the session is enough.
- **Tools that type into other sessions.** Remote Control already lets you
  answer a session yourself.
