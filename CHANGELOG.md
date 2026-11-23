# Changelog

What changed in each version of magi.
[docs/support.md](docs/support.md#versions) says what a version number
promises.

## 1.1.0

- In a machine's monitor, a click on a card opens its list: processes
  for CPU and for memory, volumes for disk, interfaces with their rates
  for network, and sensors for temperature on Linux. Each list shows up
  to 8, the busiest, largest or hottest first. A process that belongs
  to a session shows the session's title, and a click opens it. Memory
  also shows a bar split into its parts. The back button or Escape
  steps back one level.
- The menu bar icon is smaller. The Show Waiting Count setting is gone:
  a waiting session shows as the icon's badge, an orange dot with Color
  the Badge on.
- The Disk card's used space is now the disk's size minus what's
  available, so it counts space the system reserves and matches the
  volume list.
- Opening a session no longer hangs when the Mac is busy.
- Releases are signed from a GitHub environment that only `v*` tags can
  use.

## 1.0.1, 2026-11-21

- A notification now shows the app the session runs in as its picture:
  the Claude desktop app, Rex or Ghostty, or a terminal symbol for
  anything else. A session magi can't open shows none.

## 1.0.0, 2026-11-21

The first stable release. [docs/support.md](docs/support.md#versions)
lists what now stays the same until 2.0. No 0.10.0 was published.

The menu bar app, new, for macOS 26 and later.

- It lists the sessions on all your machines, the ones waiting on you
  first, each with Claude's question. A click on a row opens the session
  where it runs: the Claude desktop app, Rex, Ghostty, Terminal, a tmux
  pane, or its Remote Control link.
- A notification when a session starts waiting on you. A click on it
  opens the session. An optional sound.
- A monitor per machine: CPU, memory, temperature, disk and network over
  the last hour, each session's CPU and memory, uptime, and whether the
  machine is on battery or reached through a Tailscale relay.
- Settings has General, Notifications and Monitor tabs, with Open at
  Login, a waiting count next to the icon, and the monitor's modules and
  their order.
- "Set up magi" on a Mac installs it in `/Applications`, and
  `setup_machine` without a machine sets up only this Mac. The watcher
  keeps the app at magi's version. It is signed with magi's own
  certificate and isn't notarized.

The plugin.

- You install magi by adding the plugin in Claude Code. Its launcher
  downloads the release binary for its version and checks its SHA-256.
  The install script is gone.
- Nobody types magi commands. `init`, `try`, `doctor`, `move`, `pull` and
  `status` became the tools `setup_machine`, `move_session`, `status`,
  `unpark`, `allow_project` and `uninstall`.
- `move_session` moves another session in either direction, a quit one
  included. A pull now carries folder trust and runs the project's setup
  and services, as a push does.
- A session can be handed back to a laptop that takes no incoming SSH,
  through `magi watch` on the laptop.
- Before a move, magi lists what goes across. Claude asks you first when
  secret files, untracked files, or 50 MB or more would go.
- `exclude` now applies to untracked files in a git repository. The
  default `exclude` and `secrets` apply when the config has none.
- You approve a project's `.magi.toml` in a dialog Claude Code shows, for
  that file's SHA-256 only.
- A delegated Claude doesn't get the tools that are your call.
- magi refuses to move a session when Claude Code on either machine is a
  version its tests haven't passed with.
- Every move writes a journal, and magi settles a move that was cut off
  on its own. It never deletes a folder or a transcript.
- `magi watch` writes each machine's load, CPU, memory, disk,
  temperature and network, and each session's state, CPU, memory and
  where it runs, to `~/.magi/live/<machine>.json`, with the last hour of
  readings.
- Claude offers magi by itself for slow builds and tests, and for work
  that needs another operating system.
- A move's result gives the Remote Control link and each step's time.
  `sessions` opens with the sessions waiting on you.
- The nightly is a GitHub Actions workflow on Ubuntu and macOS runners. A
  release publishes only after CI passes on macOS, Ubuntu and Alpine,
  and it builds and signs the menu bar app.
- Removed Codex support, Rex and the `magi-overview` program. magi moves
  Claude Code sessions only, and tmux is the only launcher. An old config
  with a `codex` key still loads.

Fixed.

- Stopping a session could take the whole tmux server down, and on macOS
  could stop nothing.
- A busy session never moved, because the move held the lock the Stop
  hook needed.
- `magi arm` armed the move even when it couldn't read what goes.
- A pull failed after the session had already moved when its Remote
  Control link was slow.
- An edit made in the same second as the last git index write could be
  left out of a copy.
- Unpacking a copy could write through a symlinked folder or stop
  halfway.
- The `diff` tool failed for sessions on other machines, and an orphaned
  watch loop could be left running.

## 0.9.1, 2026-11-18

Hand back finds the machine you meant from part of its name. A delegated
Claude keeps going after one denied command.

## 0.9.0, 2026-11-18

The program became `magi`. A session can be handed back from the machine
it moved to, and handoffs work in tmux as well as in Rex.

## 0.8.0, 2026-11-18

cielago, the handoff tool, became part of magi. A background Claude's
changes are applied to your working tree when it comes back.

## 0.7.0 and 0.7.1, 2026-11-17

magi got a command-line client and an MCP server, and Claude can send
work to the machine that fits. 0.7.1 was tested with Claude Code 2.1.326.

## 0.1.0 to 0.5.0, 2026-11-14 and 2026-11-15

The first releases were a live viewer of your machines and coding agents
in a Rex pane.
