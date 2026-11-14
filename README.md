<h1 align="center">magi</h1>

<p align="center">
  <b>Every machine you own, and every coding agent on them, in one Rex pane.</b><br>
  A tiny agent on each Mac and Linux box streams live metrics over Tailscale, and one <code>magi</code> pane in <a href="https://www.superlogical.com/rex">Rex</a> shows the whole fleet: CPU, memory, disks, network, and which Claude Code sessions are working, waiting on you, or burning money.
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> ·
  <a href="#why-rex">Why Rex</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#what-it-shows">What it shows</a> ·
  <a href="#configure">Configure</a> ·
  <a href="#questions">Questions</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-2024-CE422B" alt="Rust 2024">
  <img src="https://img.shields.io/badge/Rex-pane-111111" alt="Rex pane">
  <img src="https://img.shields.io/badge/Claude%20Code-sessions-D97757" alt="Claude Code sessions">
  <img src="https://img.shields.io/badge/macOS%20·%20Linux-black" alt="macOS and Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

You run agents on more than one machine now: a long refactor on a server,
a quick fix on the laptop, something left running on a box you forgot about.
Each one might be stuck on a permission prompt, out of context, or quietly
spending. Finding out means opening every terminal.

magi puts all of it on one screen, and [Rex](https://www.superlogical.com/rex)
puts that screen on every device you own. Keep a `magi` pane in a Rex session
on a machine that's always on, then open it from your Mac, your phone or the
web: it shows every machine of yours that runs an agent, live, and every
Claude Code session on them, with the ones that need you at the top.

## What you get

1. **The whole fleet, from anywhere.** One viewer finds every machine of yours
   on Tailscale that runs a magi agent. No hub, no database, no config file.
2. **Agents that need you, first.** Sessions waiting on a permission prompt or
   your input sort to the top, with the tool they want to run.
3. **What it costs, as it happens.** Tokens and cost per session, subagents
   included, priced at Anthropic's list prices and counted once even when a
   session is forked or resumed.
4. **Context and plan limits.** An optional status line reports each session's
   context use and your 5-hour and weekly plan windows.
5. **Light enough to forget about.** The agent idles at 0% CPU and about 6 MiB,
   and samples only while someone is looking.
6. **Private by default.** The agent listens only on your Tailscale address and
   asks Tailscale who each peer is before reading a byte. Only your own
   devices get in; tagged machines are refused.

```text
agent, 32 viewers at 100 ms, 1,000 extra processes and every core busy   →   0.73% CPU · 6.9 MiB · p99 105 ms
```
<p align="center"><i>On a MacBook (Apple M4), from <code>bench/agent_load.py</code>.</i></p>

## Why Rex

magi is a terminal app you leave running, and it is only useful if you can
glance at it from wherever you are. That's what
[Rex](https://www.superlogical.com/rex) is for.

- **One live screen on every device.** A Rex session lives on the server, not
  in a window. Close the app and the `magi` pane keeps running; reconnect from
  the Mac app, the iOS app or the web and it's the same screen, already up to
  date. There is no dashboard to host and nothing to sign in to.
- **Next to the agents it watches.** Your Claude Code sessions already live in
  Rex tabs. When magi says one needs you, its tab is a switch away in the same
  app.
- **Phone-sized.** The layout is built to stay readable in the iOS app's
  narrower panes, not only on a desktop.
- **Where it's going.** Pixel charts drawn with the kitty graphics protocol
  inside the Rex pane, and alerts that reach whichever Rex client you're using.

magi works in any terminal too; without Rex you see it only where you started
it.

## How it works

Each machine runs `magi agent` as a background service (launchd on macOS, a
systemd user service on Linux). The viewer:

1. Asks `tailscale status` which of your devices are online, every 5 seconds.
2. Connects to each one's agent on port 2015, and to the local agent over a
   private Unix socket.
3. Receives a compact binary sample every second from each agent, sampled once
   and shared between all viewers.
4. Redraws only what changed, at most 30 times a second.

On each machine the agent reads Claude Code's own files:

| Source | What magi takes from it |
|---|---|
| `~/.claude/sessions/` | Which sessions are running, and whether each is working, waiting or idle. A session counts only while its process is alive with the same start time. |
| `~/.claude/projects/` | Model, tokens and cost, read from the end of each transcript and its subagents, in bounded chunks. |
| The magi status line | Context use and plan windows, which only a status line receives. |

| Part | What it does here |
|---|---|
| **[Rex](https://www.superlogical.com/rex)** | Keeps the viewer running in a session you can open from any device. |
| **Tailscale** | Finds your machines, carries the samples, and says who is connecting. |
| **Claude Code** | Its session registry, transcripts and status line are the source for everything agent-related. |

## What it shows

| | |
|---|---|
| **Machines** | CPU (per core in the detail view), memory and swap, storage, disk I/O, network, uptime and a recent-load chart. Lost machines stay listed and say so. |
| **Agents** | Each Claude Code session: title, project, state (`working · Edit`, `needs you · Bash`, `idle 4m`), model, context, tokens and cost. |
| **Plan** | Your 5-hour and weekly usage with when each resets, once the status line is installed. |

Use `↑`/`↓` (or `j`/`k`) to move between machines and agents, `g`/`G` for the
first and last, and `q` to quit.

## Install

On every machine you want to see:

```bash
cargo install --locked --git https://github.com/felitrejos/magi magi-monitor
magi agent install
```

Then, on the machines where you use Claude Code:

```bash
magi statusline install
```

And in a [Rex](https://www.superlogical.com/rex) pane on a machine that's
always on:

```bash
magi
```

Every machine needs [Tailscale](https://tailscale.com) and the `tailscale` CLI.

## Configure

There is no config file. The few settings are flags:

| Command | Flags |
|---|---|
| `magi` | `--port` (default 2015) |
| `magi agent install` | `--port`, `--interval` in ms (100–60000, default 1000) |
| `magi agent uninstall` | |
| `magi statusline install` / `uninstall` | |

`magi statusline install` changes only `statusLine` in
`~/.claude/settings.json`. It never replaces a status line you already have,
keeps a backup with the same permissions, and writes atomically.

## Questions

**Why not a central server?**
Nothing to host and nothing to keep in sync. Each agent answers for its own
machine, and every viewer asks them directly.

**Which machine should run the viewer?**
One that's always on, like a home server. Its Rex session keeps `magi` running,
and every Rex client you open shows that same pane.

**What does the cost number mean?**
What the session's tokens would cost at Anthropic's API list prices, which is
also what Claude Code's own `/cost` shows. On a Pro or Max plan you pay your
subscription, not this; use it to compare sessions. A `≥` means some of a
session's usage was on a model magi doesn't have a price for.

**Does the status line slow Claude Code down?**
No. It prints first, then hands the numbers to the agent and waits at most
300 ms, even with no agent running.

**Can other people on my tailnet see my machines?**
No. The agent asks Tailscale who each peer is and accepts only devices logged
in as you.

**Where does the data go?**
Nowhere. Samples go from the agent straight to your viewer and are never
stored, and no transcript text leaves the machine.

## Limits

- Live data only; there is no history.
- Claude Code only for now. Codex sessions come next.
- `~/.claude/sessions/` is not a documented Claude Code format, so a Claude
  Code update can change it. magi keeps working without it, but loses session
  state until it's updated.
- macOS and Linux only. Windows later.

## License

[MIT](LICENSE)
