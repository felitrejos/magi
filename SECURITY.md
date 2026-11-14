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
  its regular samples it sends counts, states, model names and session titles,
  never message text.
- For each agent process it reads that process's CPU time and memory, and
  from its environment only `REX_SESSION`, `REX_BLOCK`, `REX_SERVER` and
  `REX_BIN_DIR`. Nothing else in the environment is kept or sent.
- When a viewer selects a session that runs in a Rex pane, the agent captures
  that pane's visible screen through Rex and sends it to that viewer only,
  until it selects something else. This is the one time screen text, which
  can include the conversation, leaves the machine, and only to a viewer
  signed in as you.
- A viewer can ask an agent to do two things: watch a pane, or move a Claude
  Code session with [cielago](https://github.com/felitrejos/cielago). The
  agent acts only on sessions it found itself, checks the machine name, and
  runs `rex` and `cielago` with arguments, never through a shell.
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
