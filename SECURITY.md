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

- Local viewers, the status line and the new-session launcher (which the
  viewer opens in a Rex floating pane and talks to for one choice) use Unix
  sockets in `~/.local/state/magi/`, a folder only you can open. The viewer
  only starts a folder the launcher was offered.
- The agent reads Claude Code's and Codex's session files and transcripts. In
  its regular samples it sends counts, states, model names, session titles,
  the short description of each running subagent and the first line of each
  [cielago](cielago) task's command or prompt,
  never message text.
- When no Claude Code status line has reported your plan's usage in the
  last five minutes, the agent reads Claude Code's sign-in (the Keychain item
  `Claude Code-credentials` on macOS, `~/.claude/.credentials.json` on Linux)
  and asks `api.anthropic.com/api/oauth/usage` for it, at most every five
  minutes. It uses the access token only while it hasn't expired and never
  renews it, so Claude Code's own sign-in is left as it was. The token goes
  to `curl` on its standard input, never on a command line, and is not kept,
  logged or sent anywhere else; only the percentages and reset times are.
- When a repo the viewer hasn't seen before shows up, the viewer asks Claude
  (Haiku, through `claude -p` with no tools, no MCP servers and no saved
  session) which project it belongs to. It sends only repo names, project
  names and up to three of the new repo's session titles, once per repo; the
  repos already there when it first ran are never sent.
- When no Codex session has reported its usage limits in the last five
  minutes, the agent asks Codex itself, at most every five minutes: it runs
  `codex app-server` and sends only the `account/rateLimits/read` request,
  so Codex uses its own sign-in and magi never sees a token. The process is
  stopped after its answer or 15 seconds.
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
  that machine's folders with `cielago open`, continue a Claude Code session
  in Rex as a copy (`claude --resume --fork-session`, which leaves the
  original untouched), or move a Claude Code session with cielago. The agent
  acts only on sessions it found itself, checks the machine name, and runs
  `rex` and `cielago` with arguments, never through a shell, each with a time
  limit after which it and anything it started are stopped.
- A message goes only into the session's own Rex pane, as a bracketed paste
  through Rex's paste event, then Enter. It is at most 16,000 characters,
  keeps its lines and tabs, has every other control character removed (Rex
  also strips escapes from pastes, so a message can't end the paste early),
  and is sent only while the session is idle, so it can't land in a prompt
  that opened meanwhile. An answer is a single key (`1` to allow, `Esc` to
  deny), sent only while that session still waits on the same prompt the
  viewer saw: same tool, waiting as long. Messages and answers to one
  machine are typed one at a time. Since a message can ask the agent to do
  anything, only viewers signed in as you can send one, like everything else.
- `d` and `v` ask the agent for a session's changes (`git diff HEAD` and the
  names of new files, in the session's own folder, under 1 MB) or its
  outputs (the last plan it proposed, and the documents it wrote or edited
  inside its own folder: Markdown, text, HTML and CSV, at most 40 files and
  768 KB). The viewer writes the answer to files only you can read in
  `~/.local/state/magi/popups/`, shows them in a Rex floating pane on its
  own machine (through fzf, delta and `magi md`, which draw only from those
  files), and the pane deletes them when it closes.
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
