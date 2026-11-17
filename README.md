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

- **magi** shows every Claude Code and Codex session on your machines, and
  each machine's load, in one [Rex](https://www.superlogical.com/rex) pane.
  You still work in the agents themselves. magi puts each session in the
  right place and tells you when one needs you.
- **cielago** moves a Claude Code or Codex session to another machine, along
  with its conversation and uncommitted work. It also lets Claude send work
  there. It works on its own, from any Claude Code session, over SSH.

## magi

- **Every session, every machine.** magi is built to sit next to Rex's
  sidebar. What needs you is at the top. Below that, each machine gets a
  band with its load and one line per session saying what it's doing. It
  handles any number of machines and needs no server or config file.
- **Sessions in their place.** A new session opens on whichever machine has
  room. It goes in a Rex session named after its folder, in a tab for its
  repo. Each subagent gets its own tab beside it. That tab shows the
  subagent's work live and closes when it's done.
- **Answer from the overview.** You can allow or deny a prompt, see a
  session's diff, or jump to its pane. Everything else happens in the
  session itself.
- **Drive it from the Claude app.** Sessions that magi starts run with Remote
  Control and are named after their repo and machine. That lets the Claude
  app on your desktop or phone talk to them. `magi start` prints the link.
- **Limits and alerts.** magi shows your 5-hour and weekly windows for Claude
  and Codex. It alerts you when a session needs you, a machine goes quiet or
  a plan nearly runs out.
- **Light and private.** The agent idles at 0% CPU. Only devices signed in to
  your tailnet as you can connect.

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
magi agent install    # on every machine
magi                  # in a Rex pane
```

This installs magi and cielago. magi needs [Tailscale](https://tailscale.com)
and Rex **0.1.0-preview.20** on every machine, and it only runs in Rex. While
Rex is in preview, magi follows only its latest release. To see Claude Code's
context use and plan windows, run `magi statusline install` where you use it.

## cielago

cielago moves a Claude Code session to another machine, with or without
magi. In any Claude Code session, say "hand this off to my homelab". When the
turn ends, the conversation, branch, uncommitted changes and listed secrets
land there. The session carries on in a tab of its own and remembers
everything.

<p align="center">
  <img src="assets/cielago.gif" alt="A Claude Code session in Rex on a Mac is asked what its project does, then told to hand itself off to the homelab. cielago moves it, and the session continues on the homelab, where it still remembers the answer it gave." width="820">
</p>

- **From the CLI.** `cielago move <session> <host>` sends a session.
  `cielago pull <host>` brings one back. `cielago doctor` checks both ends.
  In magi, `m` moves the selected session to the machine you pick.
- **Codex too.** `cielago move` and `cielago pull` take Codex sessions as
  well. The conversation, branch, changes and secrets move the same way, and
  the session reopens with `codex resume` in a tab of its own. If Codex is
  in the middle of a turn, it moves when the turn ends. If a plain `codex`
  is open in the same folder, quit it first, because cielago can't tell
  which session it shows. You move a Codex session from the CLI or magi,
  since Codex has no "hand this off". The other machine needs Codex
  installed, and `cielago init` asks for its path.
- **Send work.** Claude gets tools to run a command on another machine, hand
  a task to a background Claude, or open a second session. Each one runs in
  a copy of your working tree. Claude asks you first.
- **The right machine for the job.** With magi, each machine reports its
  system, architecture and tools, including whether Docker is running. A
  Linux build started on a Mac goes to your Linux machine and runs natively.
  The fallback is a container on the machine with the most room. This
  machine is the last resort.
- **Ports follow.** A project's `.cielago.toml` names what to start when it
  arrives and which ports to forward back, so `localhost:3000` keeps working.
- **Rex or tmux.** Sessions open in Rex when it's there and in tmux when it
  isn't. Both work over plain SSH.

```bash
curl -fsSL https://raw.githubusercontent.com/felitrejos/magi/main/install.sh | sh -s cielago
cielago init          # on each machine
claude plugin marketplace add felitrejos/magi && claude plugin install cielago@magi
```

### From scripts and other tools

The same actions work from the command line. That way another orchestrator,
a script or Claude itself can use magi underneath:

```bash
magi sessions --json            # every session on every machine
magi send fix-login "now run the tests"   # queued until it's idle
magi answer fix-login yes       # or: magi choose fix-login 2
magi move fix-login homelab
magi start homelab ~/developer/stampr/stampr
magi events                     # what needs you, as JSON lines
magi subagents                  # every running subagent and the step it's on
magi recent fix-login           # its latest steps, one line each
magi wait fix-login             # returns once it's done or needs you
```

`wait` and `recent` are the cheap way for an agent to keep an eye on other
work. `wait` is one blocking command, so the agent doesn't have to check
again and again. `recent` gives a few lines per step instead of a whole
transcript. Both take a subagent too: `magi wait fix-login "find auth"`.

`magi mcp` offers the same actions as MCP tools, so Claude can look after
your other sessions from Claude Desktop, Claude Code or any MCP client. In
Claude Code, run `claude mcp add magi -- magi mcp`. In Claude Desktop, add
this to `claude_desktop_config.json`:

```json
{ "mcpServers": { "magi": { "command": "/Users/you/.local/bin/magi", "args": ["mcp"] } } }
```

The tools that only look (machines, sessions, subagents, recent work,
waiting, diffs, alerts) need no approval from the tools' side. The tools
that act (message, answer, choose, move, start) are marked as such, so your
client asks you first.

## How it works

Each machine runs `magi agent` in the background (launchd or a systemd user
service). The viewer finds the agents through Tailscale and gets a small
sample from each one every second. The agent reads the files the coding
agents already write (`~/.claude`, `~/.codex`) and follows Rex's events to
know each pane's state. When you answer a session, the agent on that machine
types your answer into the session's Rex pane.

| Agent on an M4 MacBook | CPU | Memory |
|---|---|---|
| Nobody watching | 0% | 6 MB |
| 32 viewers, every core busy | 0.7% | 7 MB |
| 32 viewers, 5 live Claude sessions | 2.9% | 12 MB |

## FAQ

**Can magi tell when Codex needs approval?** Yes, from Codex 0.164 on.
Codex reports its prompts to Rex the way Claude Code does, and magi follows
them.

**Does the status line slow Claude Code down?** No. It prints first and
gives the agent at most 300 ms. It never replaces a status line you already
have.

**What if Claude Code changes its files?** They aren't a documented format.
magi notices, says so, and falls back to what the transcripts show.

What's next is in [ROADMAP.md](ROADMAP.md). What magi and cielago read, send
and change is in [SECURITY.md](SECURITY.md). Licensed [MIT](LICENSE).
