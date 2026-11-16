<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Run your coding agents across all your machines, from one Rex pane.</b>
</p>

<p align="center">
  <a href="#magi">magi</a> ·
  <a href="#cielago">cielago</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#faq">FAQ</a>
</p>

<p align="center">
  <img src="assets/fleet.gif" alt="magi in the Rex app: machines and agents on the left; on the right a home server's CPU, memory and network as pixel charts, then Claude Code and Codex sessions, and the alerts view." width="820">
</p>

This repo holds two apps that work together or alone:

- **magi** is the overview of every Claude Code and Codex session on your
  machines, and each machine's load, in one [Rex](https://www.superlogical.com/rex)
  pane. You work in the agents themselves; magi puts each session in the
  right place and tells you when one needs you.
- **cielago** moves a Claude Code session, with its conversation and
  uncommitted work, to another machine, and lets Claude send work there. It
  works on its own, from any Claude Code session, over SSH.

## magi

- **Every session, every machine.** Sessions are grouped by project, the ones
  waiting on you on top, with each machine's CPU, memory and network beside
  them. No server or config file.
- **Sessions in their place.** A new session opens on whichever machine has
  room, in its project's Rex session and its repo's tab. Each subagent gets a
  tab of its own beside it, showing its work live, that closes when it's done.
- **Answer from the overview.** Allow or deny a prompt, switch a session's
  permission mode, see its diff and outputs, or jump to its pane.
- **Zen.** A second layout to glance at: a column per machine, its load and
  its sessions.
- **Limits and alerts.** Your Claude and Codex 5-hour and weekly windows, and
  an alert when a session needs you, a machine goes quiet or a plan nearly
  runs out.
- **Light and private.** The agent idles at 0% CPU, and only devices signed in
  to your tailnet as you can connect.

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
magi agent install    # on every machine
magi                  # in a Rex pane
```

This installs magi and cielago. magi needs [Tailscale](https://tailscale.com)
and Rex **0.1.0-preview.19** on every machine, and runs only in Rex. While Rex
is in preview, magi follows its latest release only. Run `magi statusline
install` where you use Claude Code to see its context use and plan windows.

## cielago

cielago moves a Claude Code session to another machine, with or without
magi. In any Claude Code session, say "hand this off to my homelab": when the
turn ends, the conversation, branch, uncommitted changes and listed secrets
land there, and the session carries on in a tab of its own, remembering
everything.

<p align="center">
  <img src="assets/cielago.gif" alt="A Claude Code session in Rex on a Mac is asked what its project does, then told to hand itself off to the homelab. cielago moves it, and the session continues on the homelab, where it still remembers the answer it gave." width="820">
</p>

- **From the CLI.** `cielago move <session> <host>` sends a session,
  `cielago pull <host>` brings one back, and `cielago doctor` checks both
  ends. In magi, `m` does the same.
- **Send work.** Claude gets tools to run a command elsewhere, hand a task to
  a background Claude, or open a second session, each in a copy of your
  working tree on the lightest machine. Claude asks you first.
- **Ports follow.** A project's `.cielago.toml` names what to start when it
  arrives and which ports to forward back, so `localhost:3000` keeps working.
- **Rex or tmux.** Sessions open in Rex when it's there and in tmux
  otherwise, over plain SSH.

```bash
curl -fsSL https://raw.githubusercontent.com/felitrejos/magi/main/install.sh | sh -s cielago
cielago init          # on each machine
claude plugin marketplace add felitrejos/magi && claude plugin install cielago@magi
```

### From scripts and other tools

The same actions work from the command line, so another orchestrator, a
script or Claude itself can use magi underneath:

```bash
magi sessions --json            # every session on every machine
magi send fix-login "now run the tests"   # queued until it's idle
magi answer fix-login yes       # or: magi choose fix-login 2
magi mode fix-login plan
magi move fix-login homelab
magi start homelab ~/developer/stampr/stampr --project Stampr
magi events                     # what needs you, as JSON lines
```

`magi mcp` offers the same as MCP tools, so Claude can look after your other
sessions from Claude Desktop, Claude Code or any MCP client. In Claude Code:
`claude mcp add magi -- magi mcp`. In Claude Desktop, add to
`claude_desktop_config.json`:

```json
{ "mcpServers": { "magi": { "command": "/Users/you/.local/bin/magi", "args": ["mcp"] } } }
```

Looking (machines, sessions, recent work, diffs, alerts) needs no approval
from the tools' side; acting (message, answer, mode, move, start) is marked
as such, so your client asks you first.

## How it works

Each machine runs `magi agent` in the background (launchd or a systemd user
service). The viewer finds agents through Tailscale and gets a compact sample
from each every second. The agent reads what the coding agents already write
(`~/.claude`, `~/.codex`) and follows Rex's events for each pane's state. When
you answer a session, the agent on that machine types it into the session's
Rex pane.

| Agent on an M4 MacBook | CPU | Memory |
|---|---|---|
| Nobody watching | 0% | 6 MB |
| 32 viewers, every core busy | 0.7% | 7 MB |
| 32 viewers, 5 live Claude sessions | 2.9% | 12 MB |

## FAQ

**Can magi tell when Codex needs approval?** Yes, from Codex 0.164 on: Codex
reports its prompts to Rex the way Claude Code does, and magi follows them.

**Does the status line slow Claude Code down?** No. It prints first, gives the
agent at most 300 ms, and never replaces a status line you already have.

**What if Claude Code changes its files?** They aren't a documented format.
magi notices, says so, and falls back to what the transcripts show.

What's next is in [ROADMAP.md](ROADMAP.md). What magi and cielago read, send
and change is in [SECURITY.md](SECURITY.md). Licensed [MIT](LICENSE).
