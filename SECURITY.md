# Security

magi is one program, `magi`, that you run on your own machines. It talks
to your other machines only through your own `ssh`. Its one other
connection is an HTTPS download of the tested versions list from GitHub,
described below. It runs no server and opens no network listener, except
the ports you ask it to forward, which listen on `127.0.0.1`.

## What runs where

- **The machine you start from** runs `magi` when you type a command, when
  Claude Code runs the plugin's hooks and tools, and, if you installed it,
  as the `magi watch` background service.
- **The other machine** runs short shell scripts that magi sends over SSH,
  and the session or task itself in a tmux window. If the plugin is
  installed there, Claude Code runs the same hooks on it.
- **The plugin** adds the handoff skill, two hooks and the MCP server.
  All of them only run `magi`.

## SSH

- Every connection is `ssh` to a destination from your config, with
  `BatchMode=yes`, so it never asks for a password. magi passes no host key
  options, so your SSH config and `known_hosts` decide which hosts are
  trusted.
- Connections to one machine share a control socket in `~/.magi/ssh`,
  which closes 60 seconds after its last use.
- Git objects travel with `git push` and `git fetch` over that connection.
  Files travel as a `tar` stream over it.
- `magi init` runs `tailscale status --json` if Tailscale is installed,
  only to suggest machine names.

## Moving a session

- magi copies the branch, commits, staged and unstaged changes, the
  transcript and Claude's project memory. Anything pasted into the chat
  moves with the transcript.
- Only gitignored files matching the `secrets` patterns in your config are
  copied, disk to disk. magi doesn't print their contents. Logins, SSH keys
  and credential stores never move.
- If you trust a project's folder in Claude Code on this machine, magi
  marks its copy on the other machine as trusted in `~/.claude.json` there,
  so the moved session starts without asking. It replaces that file only
  if nothing changed it meanwhile. A folder you haven't trusted here is
  never trusted there.
- `magi try` creates a `magi-try-*` folder in your first local root and
  in the host's root, with its transcripts, and removes them on both
  machines when it ends. It makes three Claude turns on your account.
- On the receiving machine it recreates your branch and uncommitted work.
  It refuses if that checkout has work or commits of its own.
- A handoff is refused when the session already runs on the destination.
  One that fails stops the new copy and leaves the session running where
  it was.
- Every move writes a journal, `~/.magi/moves/<session-id>.json`, before
  it changes anything, and removes it as its last step. A journal whose
  move is no longer running belongs to a move that was cut off.
  `magi status` and the next move of that session settle it. They contact
  the other machine over SSH for at most 15 seconds.
- The live copy is the one that took new prompts since the move started.
  If both did, it is the one still running. If neither did, it is the
  copy the session was leaving, unless the move had reached its last step.
- Recovery stops the other copy's Claude or Codex process with `TERM`. It
  closes the tmux window the move opened, but only while the window still
  carries the tag magi gave it, or the tmux session if the move created
  it. It renames that copy's transcript to
  `<transcript>.handed-off-to-<machine>`, so turns taken there are kept.
  It stops that session's port forwards, and it updates magi's own records
  in `~/.magi` and the parked mark in a git checkout.
- Recovery never deletes a folder or a transcript. When the move created
  a folder that is still there, it tells you the path and leaves it.
- If both copies took new prompts and both or neither is running, magi
  stops nothing and tells you where each copy is.
- If the other machine doesn't answer, magi changes nothing there and
  keeps the journal. It prints the journal's path, which you remove
  yourself if that machine is gone for good.
- The Stop hook (`magi hook stop`) runs the handoff, with a time limit of
  200 seconds. The prompt hook (`magi hook prompt`) refuses new input in
  the copy a session left. It reads the session id from the hook's input
  and the marker in `~/.magi/moved`, and contacts nothing.

## The watcher

`magi watch` is for a laptop that takes no incoming SSH.

- It keeps one outgoing SSH connection to each machine in your config. On
  each it runs a small loop that refreshes `~/.magi/watchers/<laptop name>`
  every second and prints the ids of hand-back requests addressed to the
  laptop.
- A request is the file `~/.magi/wanted/<session-id>.json` on the far
  machine. `magi arm` writes it there when you ask a session to go back to
  a machine that isn't in that machine's config but has refreshed its
  watcher file in the last 15 seconds. It holds the laptop's name, the
  session's process id and the note on what was running.
- The laptop then pulls the session over its own connection, the same way
  `magi pull` does, and removes the request. A request can only make the
  laptop pull a session from the machine the request is on.
- The loop deletes a request whose session process is gone.
- `magi watch install` writes a launchd agent on macOS
  (`~/Library/LaunchAgents/magi-watch.plist`, log in
  `~/Library/Logs/magi-watch.log`) or a systemd user unit on Linux
  (`~/.config/systemd/user/magi-watch.service`, log in the journal). It
  runs as you, never as root, starts at login and restarts if it stops.
  The service file holds the path of `magi` and a copy of your `PATH`.
  `magi watch uninstall` stops it and removes the file.
- The log holds machine names, short session ids, folder paths, errors and
  the output of a project's `setup` command. magi writes no conversation
  text to it.

## Sending work

- `run_elsewhere`, `delegate` and `session_elsewhere` work in a fresh copy
  under `~/.magi/tasks` on the other machine and never write to your
  checkouts there.
- Claude proposes the machine and the work, and you approve both. The
  tools that start, continue or stop work are not marked read-only, so an
  MCP client can ask before using them.
- A delegated Claude runs headless in `acceptEdits` mode by default. Other
  prompts are answered "no" unless you allowed the tool, and it never runs
  with permission checks bypassed.
- When a finished `delegate` is collected, its changes are written into
  your working tree here and kept as the branch `magi/task-<id>`. If they
  clash with edits made here since the task started, nothing is applied
  and they stay on the branch.
- `magi mcp` talks only over its standard input and output, to the program
  that started it.

## Reading machines and sessions

- To propose a machine, magi reads its system, architecture, installed
  build tools, cores, load average and memory over SSH, with `uname`,
  `sw_vers` or `/etc/os-release`, `command -v`, `docker info` or
  `podman info`, and `sysctl` or `/proc`. It counts working Claude
  sessions with `claude agents --json`. This machine is read the same way
  without SSH.
- The `sessions` tool reads `claude agents --json` and
  `~/.claude/sessions/*.json` on this machine and on each machine in your
  config, and Codex's session files where Codex is configured. The `diff`
  tool runs `git diff HEAD` and lists untracked files in a session's
  folder. Both only read.

## Tested versions

- `remote/tested.json` in this repository holds, for Claude Code and for
  Codex, the oldest version magi supports, the newest version its contract
  tests passed with, and a list of versions known to fail.
- magi downloads that file from
  `https://raw.githubusercontent.com/felitrejos/magi/main/remote/tested.json`
  and keeps it in `~/.magi/tested.json`. It downloads again when that copy
  is more than an hour old, and only for a hand off, a pull or
  `magi doctor`. The request has a 5 second limit. A file magi can't parse
  is thrown away. If the download fails, magi uses the copy it has, and
  with no copy the list built into the program.
- Before a hand off or a pull copies anything, magi runs `claude --version`
  on both machines, or `codex --version` for a Codex session. It refuses
  when either version is older than the oldest supported, on the failing
  list, or newer than the newest tested. `magi doctor` fails on the same
  versions.
- The list only widens or narrows which versions magi allows. Someone who
  could change it on GitHub, or the copy in `~/.magi`, could block your
  moves or let an untested version through. It can do nothing else. magi
  runs nothing from it and sends nothing because of it.
- A nightly job on the author's Linux server keeps the list current. It
  is a systemd user timer that runs as an ordinary user. It downloads the
  newest Claude Code and Codex from their official release sources into a
  folder of its own and checks each download against its published
  SHA-256. It runs the contract tests with them. When they pass, it raises
  the newest tested version in the file and pushes that commit to `main`.
  It never writes the failing list and never touches the `claude` or
  `codex` installed on that server.

## Project files and ports

- A project's `.magi.toml` runs only after you approve that exact file with
  `magi allow`. Any edit needs a new approval.
- Its `ports` are forwarded from the machine a session left with
  `ssh -N -L`, bound to `127.0.0.1`, so other devices on that network
  can't use them. `magi forwards` lists them, `magi forwards close` stops
  one, and bringing the session back stops it too.

## Files magi writes

Settings are in `~/.config/magi/config.toml`. State is in `~/.magi`.

| Path in `~/.magi` | What it holds |
|---|---|
| `armed/` | A session set to move when its turn ends, and a lock while it moves |
| `wanted/` | Hand-back requests waiting for a watcher |
| `watchers/` | One file per laptop watching this machine |
| `watch.lock` | Keeps a second `magi watch` from starting |
| `moved/` | Where each session that left went |
| `handoffs.jsonl` | Time, session id, machines and folders of each handoff |
| `moves/` | One journal per move in progress or cut off, kept until the move is settled |
| `forwards/` | Forwarded ports and the process forwarding them |
| `tabs/`, `setup/` | The tmux window of a moved session and its setup state |
| `tasks/` | Task records and logs here, working tree copies on the other machine |
| `allowed` | Hashes of the `.magi.toml` files you approved |
| `folders` | Folders magi copied here for projects that aren't git repositories, with a fingerprint of each |
| `ssh/` | SSH control sockets |
| `tested.json` | The downloaded list of Claude Code and Codex versions magi was tested with |

Outside those, magi writes the project and session files it moves, the
trust entry in `~/.claude.json` and the watcher's service file and log.

## Releases

Release builds come from GitHub Actions with actions pinned to commit
hashes. `install.sh` downloads over HTTPS and checks the archive's SHA-256
before installing. It also removes the service and binaries of
`magi-overview`, an earlier companion program, if it finds them.

## Reporting a problem

Please report security issues privately through GitHub's "Report a
vulnerability" on this repository rather than in a public issue.
