# Security

## Network

- The agent listens on your Tailscale IPv4 address only, never on other
  interfaces.
- Before reading anything from a connection it asks `tailscale whois` who the
  peer is, and accepts only devices signed in as the same user as the agent's
  machine. Tagged devices are refused.
- Traffic is encrypted by Tailscale (WireGuard). magi adds no other transport
  and contacts nothing else.
- Frames are size-checked before allocation, handshakes have a deadline, and an
  agent serves at most 32 connections at once.

## On each machine

- Local viewers and the status line use Unix sockets in
  `~/.local/state/magi/`, a folder only you can open.
- The agent reads Claude Code's and Codex's session files and transcripts. In
  its regular samples it sends counts, states, model names, session titles,
  the short description of each running subagent and the first line of each
  [cielago](https://github.com/felitrejos/cielago) task's command or prompt,
  never message text.
- For each agent process it reads that process's CPU time and memory, and
  from its environment only `REX_SESSION`, `REX_BLOCK`, `REX_SERVER` and
  `REX_BIN_DIR`. Nothing else in the environment is kept or sent.
- When a viewer selects a session, the agent sends that one session's live
  view to that viewer only, until it selects something else: the visible
  screen of its Rex pane, or for a session outside Rex, such as one in the
  Claude desktop app, its latest messages and tool calls from the end of its
  transcript. This is the only time conversation text leaves the machine, and
  only to a viewer signed in as you.
- A viewer can ask an agent to do two things: watch a session, or move a
  Claude Code session with cielago. The agent acts only on sessions it found
  itself, checks the machine name, and runs `rex` and `cielago` with
  arguments, never through a shell, each with a time limit after which it and
  anything it started are stopped.
- `magi statusline install` edits only `statusLine` in
  `~/.claude/settings.json`, writes atomically, keeps the file's permissions
  and leaves a backup with the same permissions.
- The only file magi writes on its own is the alert history
  (`~/.local/state/magi/alerts.jsonl`, readable only by you).

## Releases

Release builds come from GitHub Actions with actions pinned to commit hashes.
`install.sh` downloads over HTTPS and checks each archive's SHA-256 before
installing.

## Reporting a problem

Please report security issues privately through GitHub's "Report a
vulnerability" on this repository rather than in a public issue.
