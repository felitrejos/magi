# Security

magi is one program, `magi`, that you run on your own machines. It talks
to your other machines only through your own `ssh`. It runs no server and
opens no network listener, except the ports you ask it to forward, which
listen on `127.0.0.1`.

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
- On the receiving machine it recreates your branch and uncommitted work.
  It refuses if that checkout has work or commits of its own.
- A handoff is refused when the session already runs on the destination.
  One that fails or is interrupted stops the new copy and leaves the
  session running where it was. If the new copy couldn't be stopped, magi
  records it in `~/.magi/strays`. `magi status` and the next handoff of
  that session then stop it over SSH and close its tmux window, but only
  while the window still carries the tag magi gave it.
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
| `strays/` | Copies a failed handoff couldn't stop |
| `forwards/` | Forwarded ports and the process forwarding them |
| `tabs/`, `setup/` | The tmux window of a moved session and its setup state |
| `tasks/` | Task records and logs here, working tree copies on the other machine |
| `allowed` | Hashes of the `.magi.toml` files you approved |
| `folders` | Folders magi copied here for projects that aren't git repositories, with a fingerprint of each |
| `ssh/` | SSH control sockets |

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
