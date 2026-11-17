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
  to the machine that fits it, such as running your CI on the Linux machine
  that matches it before you push. It works on its own, from any Claude
  Code session, over SSH.

## magi

- **Every session, every machine.** magi is built to sit next to Rex's
  sidebar. What needs you is at the top. Below that, each machine has a row
  with its load, and its sessions are listed under it, one line each,
  saying what each is doing. It handles any number of machines and needs no
  server or config file.
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

cielago does two things, with or without magi: it moves a session to another
machine, and it lets Claude send work to another machine while you keep
working here.

To move a session, say "hand this off to my homelab" in any Claude Code
session. When the turn ends, the conversation, branch, uncommitted changes
and listed secrets land there. The session carries on in a tab of its own
and remembers everything.

<p align="center">
  <img src="assets/cielago.gif" alt="A Claude Code session in Rex on a Mac is asked what its project does and explains a small Python library. Told to hand this off to the homelab, cielago moves it when the turn ends. The session opens in a tab on the homelab, and asked what it just said, it recaps its earlier answer." width="820">
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
- **Ports follow.** A project's `.cielago.toml` names what to start when it
  arrives and which ports to forward back, so `localhost:3000` keeps working.
- **Rex or tmux.** Sessions open in Rex when it's there and in tmux when it
  isn't. Both work over plain SSH.

To send work, ask for it in plain words: "Before I push, have an agent on
the machine that matches our CI run it and fix anything that fails. Tell me
when it's done."

<p align="center">
  <img src="assets/subagent-ci.gif" alt="In a Claude Code session on a Mac, the user asks for an agent on the machine that matches CI to run it and fix what fails. Claude reads the CI workflow, asks cielago for a Linux x86_64 machine and proposes the homelab, and the user agrees. A Claude starts on the homelab in its own copy of the repo, in a Rex tab named Pre-push CI. magi shows the Mac session's work running on the homelab. When the homelab Claude finishes, the Mac session is woken and reports one failing test, fixed, with CI passing and the fix on a branch to review." width="820">
</p>

Here Claude reads the CI workflow, sees it needs Linux on x86_64 with
Python 3.12, and proposes the homelab. A Claude starts there, runs CI, fixes
the test that fails, and the fix comes back as a branch to review before you
push. Nothing runs on the Mac except Claude asking and reporting.

- **Send work.** Claude gets tools to run a command on another machine, hand
  a task to a background Claude, or open a second session. Each one runs in
  its own copy of your working tree, in a tab you can watch, and never
  touches your checkout. A background Claude shows in Rex as Claude Code,
  and a plain command shows as a process.
- **You agree to the machine.** Claude always proposes the machine and says
  what will run there, then waits for your answer. It asks even when you
  tell it to pick one.
- **The right machine for the job.** cielago knows each machine's system,
  architecture and tools, including whether Docker or Podman is running.
  It gets them from magi, along with each machine's live load, or reads
  them over SSH when magi isn't there. A machine that runs the work
  natively comes first, so a Linux build started on a Mac goes to your
  Linux machine. If several can, the least busy one wins. If none can, the
  work runs in a Docker or Podman container on whichever machine is least
  busy, this one included. Work that needs nothing in particular goes to
  the first machine in your list with room.
- **Told when it's done.** Claude runs `cielago tasks wait <id>` in the
  background. It blocks until the task finishes, so the session is woken
  then instead of checking again and again, and Claude collects the result.
  A background Claude's changes come back as the branch `cielago/task-<id>`,
  without touching your working tree.
- **See it in magi.** In the demo, the Mac session's line shows `⇢ homelab`
  while its work runs there, and the Claude on the homelab shows
  `← Pre-push CI`, the session that sent it.
- **Track it from the CLI.** `cielago tasks` lists the work you've sent and
  its state. `cielago tasks show <id>` prints a task's output, `cielago
  tasks wait <id>` waits for it to finish, and `cielago tasks cancel <id>`
  stops it and removes its copy.

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
