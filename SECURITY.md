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
  [cielago](cielago) task's command or prompt,
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
- A viewer can ask an agent to watch a session, message it, answer its
  permission prompt or question, stop its turn, start a new session in one of
  that machine's folders with `cielago open`, or move a Claude Code session
  with cielago. The agent
  acts only on sessions it found itself, checks the machine name, and runs
  `rex` and `cielago` with arguments, never through a shell, each with a time
  limit after which it and anything it started are stopped.
- A message is typed only into the session's own Rex pane, as one line of at
  most 2,000 characters with control characters removed, and only while the
  session is idle, so it can't land in a prompt that opened meanwhile. An
  answer is a single key (`1` to allow, `Esc` to deny), sent only while that
  session still waits on the same prompt the viewer saw: same tool, waiting
  as long. Messages and answers to one machine are typed one at a time. Since
  a message can ask the agent to do anything, only viewers signed in as you
  can send one, like everything else.
- `magi statusline install` edits only `statusLine` in
  `~/.claude/settings.json`, writes atomically, keeps the file's permissions
  and leaves a backup with the same permissions.
- The only files magi writes on its own are the alert history
  (`~/.local/state/magi/alerts.jsonl`) and your last plan windows
  (`~/.local/state/magi/plans.json`), both readable only by you.

## Moving work with cielago

- Everything travels over your own `ssh` connection: git objects with
  `git push`/`git fetch`, files with `tar` over the same connection. cielago
  runs no server and sends nothing anywhere else.
- Only gitignored files matching your `secrets` patterns are copied, disk to
  disk; their contents never enter the conversation, output or logs. The
  transcript and Claude's memory move as files, so anything pasted into the
  chat moves with them. Logins, ssh keys and credential stores never move.
- If you trust a project's folder in Claude Code on this machine, cielago
  marks its copy on the other machine as trusted in `~/.claude.json` there, so
  the moved session starts without asking. It writes that file atomically,
  readable only by you, and only if nothing changed it meanwhile. A folder you
  haven't trusted here is never trusted there.
- On the receiving machine it recreates your branch and uncommitted work, but
  refuses if that checkout has work or commits of its own, and only creates
  folders that don't exist yet.
- `run_elsewhere`, `delegate` and `start_session` work in a fresh copy under
  `~/.cielago/tasks` there and never write to your checkouts. Claude proposes
  the machine and the work, and you approve both. A delegated Claude runs in
  `acceptEdits` mode with other prompts answered "no", never with permission
  checks bypassed.
- A project's `.cielago.toml` runs only after you approve that exact file with
  `cielago allow`; any edit needs a new approval.
- Its `ports` are forwarded over your own SSH connection from the machine a
  session left, listening on `127.0.0.1` only, so other devices on that
  network can't use them. `cielago forwards` lists them, `cielago forwards
  close` stops one, and bringing the session back stops it too.

## Releases

Release builds of magi and cielago come from GitHub Actions with actions
pinned to commit hashes.
`install.sh` downloads over HTTPS and checks each archive's SHA-256 before
installing.

## Reporting a problem

Please report security issues privately through GitHub's "Report a
vulnerability" on this repository rather than in a public issue.
