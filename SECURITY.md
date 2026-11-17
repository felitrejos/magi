# Security

magi has two layers. Moving work runs over your own SSH connection. The
overview adds an agent on each machine that viewers reach over Tailscale.

## The overview's network

- The agent listens on your Tailscale IPv4 address only, never on other
  interfaces.
- Before reading anything from a connection it asks `tailscale whois` who the
  peer is, and accepts only devices signed in as the same user as the agent's
  machine. Tagged devices are refused.
- Traffic is encrypted by Tailscale (WireGuard). The overview adds no other
  transport. Beyond your machines it contacts only the services named
  below, Anthropic's usage endpoint and GitHub through your own `gh`.
- Frames are size-checked before allocation, handshakes have a deadline, and an
  agent serves at most 32 connections at once.

## The overview on each machine

- The command-line client (`magi sessions`, `send`, `answer` and the rest)
  and the overview tools of `magi mcp` connect exactly as the viewer does,
  so they reach only the agents a viewer could, and can do only what a
  viewer can. `magi mcp` talks only over its standard input and output, to
  the program that started it, and marks every tool that types into a
  session as not read-only, so MCP clients ask before using it. Its
  `recent_work` tool, like `magi recent`, hands that program a session's or
  subagent's messages and tool calls, which the viewer's samples never
  carry.
- Local viewers, the status line and the new-session launcher (which the
  viewer opens in a Rex floating pane and talks to for one choice) use Unix
  sockets in `~/.local/state/magi/`, a folder only you can open. The viewer
  only starts a folder the launcher was offered.
- The agent reads Claude Code's and Codex's session files and transcripts. In
  its regular samples it sends counts, states, model names, session titles,
  the short description of each running subagent and the first line of the
  command or prompt of each task sent to another machine, never message
  text.
- When no Claude Code status line has reported your plan's usage in the
  last five minutes, the agent reads Claude Code's sign-in (the Keychain item
  `Claude Code-credentials` on macOS, `~/.claude/.credentials.json` on Linux)
  and asks `api.anthropic.com/api/oauth/usage` for it, at most every five
  minutes. It uses the access token only while it hasn't expired and never
  renews it, so Claude Code's own sign-in is left as it was. The token goes
  to `curl` on its standard input, never on a command line, and is not kept,
  logged or sent anywhere else. Only the percentages and reset times are.
- When no Codex session has reported its usage limits in the last five
  minutes, the agent asks Codex itself, at most every five minutes. It runs
  `codex app-server` and sends only the `account/rateLimits/read` request,
  so Codex uses its own sign-in and magi never sees a token. The process is
  stopped after its answer or 15 seconds.
- For each agent process it reads that process's CPU time and memory, and
  from its environment only `REX_SESSION`, `REX_BLOCK`, `REX_SERVER` and
  `REX_BIN_DIR`. Nothing else in the environment is kept or sent.
- For a session on a git branch, the agent runs `gh pr view` in its folder,
  at most every two minutes per branch, for that branch's pull request state,
  checks and review. It uses `gh`'s own sign-in, and does nothing if `gh`
  isn't installed.
- Conversation text leaves the machine only when `magi recent` or `magi
  mcp`'s `recent_work` asks for it. That is one session's or subagent's
  latest messages and tool calls, or the visible screen of its Rex pane,
  sent only to a client signed in as you.
- A viewer can ask an agent to message a session, answer its permission
  prompt or question, stop its turn, start a new session in one of that
  machine's folders with `magi open`, continue a Claude Code session in
  Rex as a copy (`claude --resume --fork-session`, which leaves the original
  untouched), move a Claude Code or Codex session to another machine, or
  close a port forward whose session now runs on another machine (`magi
  forwards close`, which a viewer asks for once that's been true for a
  minute). On its own, the agent opens a read-only tab beside a session for
  each running subagent, running `magi follow`, which closes when the
  subagent finishes. For 90 seconds after starting a session it also
  watches that pane for Claude's folder-trust question, so it can be
  answered from the overview. The agent
  acts only on sessions it found itself, checks the machine name, and runs
  `rex` and `magi-remote` with arguments, never through a shell, each with a time
  limit after which it and anything it started are stopped.
- A message goes only into the session's own Rex pane, as a bracketed paste
  through Rex's paste event, then Enter. It is at most 16,000 characters,
  keeps its lines and tabs, has every other control character removed (Rex
  also strips escapes from pastes, so a message can't end the paste early),
  and is sent only while the session is idle, so it can't land in a prompt
  that opened meanwhile. An answer is a single key (`1` to allow, `Esc` to
  deny, a digit to choose), sent only while that session still waits on the
  same prompt the viewer saw, same tool, waiting as long. Trusting a folder
  is `↓` then Enter (or Enter if Yes is already selected), sent only while
  the pane still shows the trust question. Messages and answers to one
  machine are typed one at a time. Since a message can ask the agent to do
  anything, only viewers signed in as you can send one, like everything else.
- `v` asks the agent for a session's changes (`git diff HEAD` and the
  names of new files, in the session's own folder, under 1 MB). The viewer
  writes the answer to files only you can read in
  `~/.local/state/magi/popups/`, shows them in a Rex floating pane on its
  own machine (through fzf and delta, which draw only from those files),
  and the pane deletes them when it closes.
- `magi statusline install` edits only `statusLine` in
  `~/.claude/settings.json`, writes atomically, keeps the file's permissions
  and leaves a backup with the same permissions.
- The files magi keeps, all in `~/.local/state/magi/` and readable only by
  you, are the alert history (`alerts.jsonl`), your last plan windows
  (`plans.json`) and, while a diff is open, its `popups/`.

## Moving work

- Your work travels over your own `ssh` connection, git objects with
  `git push`/`git fetch` and files with `tar` over the same connection. When
  Rex is there, magi opens tabs and forwards ports through Rex's own
  connection to that machine. Moving work runs no server and sends nothing
  anywhere else.
- Only gitignored files matching your `secrets` patterns are copied, disk to
  disk. Their contents never enter the conversation, output or logs. The
  transcript and Claude's memory move as files, so anything pasted into the
  chat moves with them. Logins, ssh keys and credential stores never move.
- If you trust a project's folder in Claude Code on this machine, magi
  marks its copy on the other machine as trusted in `~/.claude.json` there, so
  the moved session starts without asking. It writes that file atomically,
  readable only by you, and only if nothing changed it meanwhile. A folder you
  haven't trusted here is never trusted there.
- On the receiving machine it recreates your branch and uncommitted work, but
  refuses if that checkout has work or commits of its own, and only creates
  folders that don't exist yet.
- `run_elsewhere`, `delegate` and `session_elsewhere` work in a fresh copy
  under `~/.magi/tasks` there and never write to your checkouts on that
  machine. When this session collects a finished `delegate`, its changes are
  written into your working tree here and kept as the branch
  `magi/task-<id>`. If they clash with edits made here since the task
  started, nothing is applied and they stay on the branch. Claude then
  merges them in itself and tells you. Ask to review first and they stay on
  the branch only. `run_elsewhere` brings back output, and a
  `session_elsewhere` still comes back as a branch. Claude proposes the
  machine and the work, and you approve both. A delegated Claude runs in
  `acceptEdits` mode by default, with other prompts answered "no", never
  with permission checks bypassed.
- To choose a machine that the overview doesn't report, magi reads its
  system, architecture and installed tools over SSH (`uname`,
  `/etc/os-release`, `command -v`, and `docker info` or `podman info`). It
  only reads.
- A project's `.magi.toml` runs only after you approve that exact file with
  `magi allow`. Any edit needs a new approval. An old `.cielago.toml` is
  read the same way.
- Its `ports` are forwarded from the machine a session left. When the
  session's tab is in Rex, magi uses `rex forward` and the traffic goes
  over Rex's connection to that machine. Otherwise it runs `ssh -N -L` over
  your own SSH connection. Both listen on `127.0.0.1` only, so other
  devices on that network can't use them. `magi forwards` lists them,
  `magi forwards close` stops one, and bringing the session back stops
  it too.
- magi keeps its state in `~/.magi` and its settings in
  `~/.config/magi/config.toml`. If a machine still has `~/.cielago` or
  `~/.config/cielago` and not the new folder, magi renames it on first
  run, on the machines it connects to as well.

## Releases

Release builds come from GitHub Actions with actions pinned to commit
hashes. `install.sh` downloads over HTTPS and checks each archive's SHA-256
before installing.

## Reporting a problem

Please report security issues privately through GitHub's "Report a
vulnerability" on this repository rather than in a public issue.
