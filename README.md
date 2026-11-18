<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Move a live Claude Code session to another machine and back, with the conversation and your uncommitted work.</b>
</p>

<p align="center">
  <a href="#moving-work-between-machines">Moving work</a> ·
  <a href="#the-overview">The overview</a> ·
  <a href="#install">Install</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#faq">FAQ</a>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="A Claude Code session in Rex on a Mac runs a small Python server on localhost:3000 that replies hi from felipes-macbook-pro. A pane under the session curls the Mac's localhost:3000 every second. The user types hand this off to my homelab. When the turn ends, the session moves to the homelab with the server and its uncommitted edit, and the Mac's localhost:3000 now answers hi from homelab. Asked there what it changed before the move and whether it is committed, Claude remembers the change and says it is uncommitted. Told to hand this back to the Mac and start the server there, the session returns, and the Mac's localhost:3000 says hi from felipes-macbook-pro again." width="820">
</p>

<p align="center">
  <i>A session moves to the homelab and back, and the Mac's localhost:3000 follows it both ways.</i>
</p>

magi moves the Claude Code session you're in to another machine.

- **It follows you.** From laptop to server and back, with its history and
  uncommitted work. Remote Control keeps it in the Claude app on your phone.
- **Work Claude sends away comes back** to your working tree by itself.
- **The overview** is the layer for [Rex](https://www.superlogical.com/rex)
  users, one pane with every machine and session.

## Moving work between machines

<p align="center">
  <img src="assets/background-work.gif" alt="In a Claude Code session on a Mac, in a small link shortener project, the user asks for a background Claude to build a linux/amd64 Docker image on a machine that runs amd64 natively, to keep going until /health answers from the container, and meanwhile to add a /stats route on the Mac. Claude proposes the homelab and notes the weekly Claude plan is at 94%, and the user picks the homelab anyway. The Mac session writes /stats and its tests while a background Claude on the homelab writes a Dockerfile, builds the image and checks /health in the container. The overview shows both. When it finishes, the Dockerfile and .dockerignore land in the Mac's working tree by themselves, and git status shows the four uncommitted changes." width="820">
</p>

<p align="center">
  <i>The homelab builds the Docker image while the Mac session keeps working, and the new files land back on the Mac by themselves.</i>
</p>

- **Hand off.** Say "hand this off to my homelab" in a Claude Code session.
  When the turn ends, the conversation, branch, uncommitted changes and
  listed secrets land there in a tab of its own. Say "hand this back to my
  Mac" there to return it, or without the overview run `magi pull MACHINE`.
  A session that left from Claude Desktop comes back into Desktop, in the
  same sidebar entry, with what it did on the other machine. `magi move`
  goes either way, Codex too. `magi doctor` checks both ends.
- **Send work.** Claude can run a command on another machine, hand a task
  to a background Claude, or open a second session. Each runs in its own
  copy of your working tree, in a tab you can watch.
- **Claude asks first.** It picks the machine by what the work needs, one
  that runs it natively or else a Docker or Podman container on the least
  busy one. Then it proposes the machine and waits for your yes.
- **Work comes back.** Claude runs `magi tasks wait <id>` in the
  background, so the session is woken when the work finishes, and `magi
  tasks` lists what you've sent. A background Claude's changes land in your
  working tree, or stay on the branch `magi/task-<id>` if you ask to review
  first. If they clash with your edits, nothing is overwritten. Claude
  merges both and tells you.
- **Project setup.** A project's `.magi.toml` names what to start on arrival
  and which ports to forward back, so `localhost:3000` keeps working.
- **Rex if you have it.** Sessions and tasks open as Rex tabs, the app
  switches to the new one, a background Claude shows as Claude Code, and
  Rex forwards the ports. Without Rex, it falls back to tmux and ssh.

## The overview

- **Every session, every machine.** What needs you is at the top. Below,
  each machine has a row with its load, then its sessions, one line each.
- **It suggests moves.** When a machine is busy and another has room, magi
  says so under the session using it. `m` moves it. Nothing moves by itself.
- **Sent work stays in view.** A session's line shows `⇢ homelab` while
  work it sent runs there. The Claude doing it shows `← Pre-push CI`, the
  session that sent it.
- **Answer from the overview.** Allow or deny a prompt, see a session's
  diff, or jump to its pane.
- **Limits and alerts.** Your 5-hour and weekly windows for Claude and
  Codex, and an alert when a session needs you, a machine goes quiet or a
  plan nearly runs out.
- **New sessions.** One opens on whichever machine has room. Each subagent
  gets a tab beside it that closes when it's done. Sessions run with Remote
  Control for the Claude app, and `magi-overview start` prints the link.
- **Better picks.** Machine load already comes over SSH. The overview adds
  your plan's usage and holds back Claude work when it's nearly used up.
- **Light and private.** The agent idles at 0% CPU. Only devices signed in
  to your tailnet as you can connect.

## Install

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
magi init             # on each machine
claude plugin marketplace add felitrejos/magi && claude plugin install magi@magi
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh -s overview
magi-overview agent install    # for the overview, on every machine
magi-overview                  # for the overview, in a Rex pane
```

`magi` needs only SSH, and the plugin adds the handoff skill, the hooks and
the MCP tools. The overview is a second program for Rex users. It needs
[Tailscale](https://tailscale.com) and Rex **0.1.0-preview.20** on every
machine, tracks Rex's latest preview and gets no new features for now.
`magi-overview statusline install` shows context use and plan windows.

## From scripts and other tools

```bash
magi-overview sessions --json   # every session on every machine
magi-overview send fix-login "now run the tests"   # queued until it's idle
magi-overview answer fix-login yes   # or: magi-overview choose fix-login 2
magi-overview move fix-login homelab
magi-overview start homelab ~/developer/stampr/stampr
magi-overview events            # what needs you, as JSON lines
magi-overview subagents         # every running subagent and its step
magi-overview recent fix-login  # its latest steps, one line each
magi-overview wait fix-login    # or a subagent: wait fix-login "find auth"
```

`magi mcp` is one MCP server. It lists machines and sessions, shows a diff
and sends work over SSH, and offers the overview's tools only when
`magi-overview` is installed. Tools that act are marked, so your client
asks first. In Claude Desktop, add this to `claude_desktop_config.json`.

```json
{ "mcpServers": { "magi": { "command": "/Users/you/.local/bin/magi", "args": ["mcp"] } } }
```

## How it works

A handoff runs over SSH when the turn ends, and a failed one leaves the
session running where it was. In a scripted test between a MacBook and a
Linux machine, one session handed itself to the Linux machine and back 20
times with Rex and 20 times with tmux. All 40 round trips worked, at a
median of 10 to 14 seconds each way, and three injected failures (the
destination already running the session, the hook killed partway, SSH
dropped mid-move) each ended with exactly one live copy and the
uncommitted work intact.

For the overview, each machine runs `magi-overview agent` in the background
(launchd or a systemd user service). The viewer finds the agents through
Tailscale and samples each one every second. The agent reads what the
coding agents already write (`~/.claude`, `~/.codex`), follows Rex's events
and types your answers into the session's Rex pane.

| Agent on an M4 MacBook | CPU | Memory |
|---|---|---|
| Nobody watching | 0% | 6 MB |
| 32 viewers, every core busy | 0.7% | 7 MB |
| 32 viewers, 5 live Claude sessions | 2.9% | 12 MB |

## FAQ

**Can magi tell when Codex needs approval?** Yes, from Codex 0.164 on.
Codex reports its prompts to Rex the way Claude Code does, and magi follows
them.

**How do I move a Codex session?** With `magi move`, `magi pull` or `m` in
the overview. It moves when its turn ends and reopens with `codex resume`.
If a plain `codex` is open in the same folder, quit it first, because magi
can't tell which session it shows. The other machine needs Codex installed,
and `magi init` asks for its path.

**Does the status line slow Claude Code down?** No. It prints first and
gives the agent at most 300 ms. It never replaces a status line you already
have.

**What if Claude Code changes its files?** They aren't a documented format.
magi notices, says so, and falls back to what the transcripts show.

What's next is in [ROADMAP.md](ROADMAP.md). What magi reads, sends and
changes is in [SECURITY.md](SECURITY.md). Licensed [MIT](LICENSE).
