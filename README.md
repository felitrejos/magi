<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Run your coding agents across all your machines, from one Rex pane.</b><br>
  Every Claude Code and Codex session on your tailnet, what each one costs and uses, a way to talk to them, and a way to move them to whichever machine has room.
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> ·
  <a href="#why-rex">Why Rex</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#alerts">Alerts</a> ·
  <a href="#move-work-between-machines">Move work</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="#roadmap">Roadmap</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-2024-CE422B" alt="Rust 2024">
  <img src="https://img.shields.io/badge/Rex-pane-111111" alt="Rex pane">
  <img src="https://img.shields.io/badge/Claude%20Code%20·%20Codex-sessions-D97757" alt="Claude Code and Codex sessions">
  <img src="https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows%20soon-black" alt="macOS and Linux, Windows soon">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <img src="assets/fleet.gif" alt="magi in the Rex app: a machines panel and an agents panel on the left, a home server's CPU, memory and network drawn as pixel charts on the right, then the detail of a Codex session and the alerts view with a plan nearly used and a session waiting on a permission prompt." width="820">
</p>
<p align="center">
  <i>A Mac and a home server, six agent sessions, one waiting on you.</i>
</p>

Agents now run on several machines at once, and each can stall on a prompt,
fill its context or quietly spend. magi is a multiplexer for them: one view of
every session and every machine's load, so a glance answers "is anything
stuck, what is it costing, and where is there room?", and you can answer,
message or move a session without leaving it.

## Highlights

- **Whole fleet, no setup.** Every machine of yours on Tailscale running a
  magi agent shows up on its own. No server, database or config file.
- **Agents first.** Sessions waiting on you rise to the top with the tool
  they want to run; the rest show what they're doing, their model, context and
  cost.
- **Watch any agent.** Select a session and it shows live beside the list,
  from whichever machine it's on: its screen if it runs in Rex, its latest
  messages if it runs elsewhere, such as the Claude desktop app. Messages are
  drawn as Markdown with highlighted code in your terminal's colours, and each
  tool call is one line, with the lines an edit added and removed.
- **Talk to it.** For a session in Rex, press `i` to send it a message, and
  `y` or `n` to answer the permission prompt it's waiting on, from any machine.
- **Spread the load.** Each machine's CPU, memory and disks sit next to what
  every session uses. When one machine is busy and another idle, magi suggests
  moving the heavy session, and cielago moves it.
- **Calm by default.** Sessions waiting on you come first, the rest sit under
  their machine with what they started nested beneath, and sessions idle for
  a day fold away.
- **What each one uses.** CPU and memory for every session and everything it
  started, such as test runs and builds.
- **Plan limits.** Your Claude and Codex 5-hour and weekly windows, with when
  they reset.
- **Alerts.** Raised in Rex and kept in a short history.
- **Cheap.** The agent idles at 0% CPU and only samples while someone watches.
- **Private.** Only devices signed in to your tailnet as you can connect.

## Why Rex

magi is meant to stay open. [Rex](https://www.superlogical.com/rex) keeps it
running on an always-on machine and lets you open that same pane from the Mac
app, the iOS app or the web, already up to date. Alerts use Rex's
[Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status),
so they show on Rex's session picker and tab headers on whichever device you
have open, right beside the tabs your agents run in. Select an agent and its
Rex pane shows live in magi; press `Enter` and Rex opens that tab, on
whichever machine.

magi runs in other terminals too. Charts and meters are drawn with the
[kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/),
placed through its Unicode placeholders, so they stay pixel-sharp wherever that
part is supported. magi asks the terminal when it starts and uses block
characters if not. iTerm2 and WezTerm show kitty images but not placeholders,
so they get blocks. Colours come from your terminal's theme, light or dark,
and follow it when you switch.

| | Rex | Ghostty, kitty 0.28+ | Other terminals |
|---|---|---|---|
| Pixel charts | ✓ | ✓ | Block characters |
| Alerts in the terminal's own UI | ✓ | – | – |
| Same pane on Mac, iOS and web | ✓ | – | – |

Every terminal keeps the alert history (`a`).

## How it works

Each machine runs `magi agent` in the background (launchd on macOS, a systemd
user service on Linux). The viewer finds agents through `tailscale status`,
connects to each on port 2015, and gets one compact sample per second,
redrawing only what changed.

The agent only reads what the coding agents already write:

| Source | Gives magi |
|---|---|
| `~/.claude/sessions/` | Running Claude Code sessions and whether each is working, waiting or idle |
| `~/.claude/projects/` | Model, tokens and cost, subagents included |
| `magi statusline` | Claude's context use and plan windows |
| `~/.codex/sessions/` | Running Codex sessions with model, tokens, cost, context and plan windows |

When you message a session or answer its prompt, the agent on that machine
types it into the session's own Rex pane.

Cost is priced from token counts at Anthropic's and OpenAI's list prices, the
same figure Claude Code's `/cost` shows, and a forked session counts shared
history once.

## Alerts

| When | Shows in Rex as |
|---|---|
| A Claude Code session waits on a permission prompt or your answer | blocked |
| A machine stops answering for 30 s | error |
| CPU stays over 90%, memory over 92% or a disk over 95% for a minute | error |
| A 5-hour or weekly plan window passes 90% | error |

Alerts clear themselves when the condition ends. Press `a` to see the last 200,
kept in `~/.local/state/magi/alerts.jsonl`.

## Move work between machines

[cielago](cielago) comes with magi. It moves a Claude Code session to another
machine, conversation and uncommitted work included, and lets Claude send work
there. magi shows where there's room and what each session costs; cielago does
the moving.

| In magi | With cielago |
|---|---|
| A session pushes a busy machine while another sits idle, and its detail suggests moving it | Press `m`, confirm with `y`, and the session continues on the other machine, each step shown in magi |
| Every machine's load and every session's CPU and memory, in `magi fleet --json` | Claude picks the lightest machine for `run_elsewhere`, `delegate` and `start_session`, and asks you first |
| Your plan windows | cielago holds back new work when a window is nearly used |

Run `cielago init` once per machine to set it up; see its
[README](cielago/README.md). Without it, magi works the same and `m` explains
how to get it.

## Install

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
magi agent install          # on every machine to monitor
magi statusline install     # where you use Claude Code
magi                        # in a Rex pane on an always-on machine
cielago init                # optional: to move sessions and work between machines
```

The script picks the build for your system (macOS arm64/x86_64, Linux
x86_64/arm64), checks its SHA-256 and installs `magi` and `cielago` to
`~/.local/bin`. From source:
`cargo install --locked --git https://github.com/felitrejos/magi magi-monitor`
and `go install github.com/felitrejos/magi/cielago@latest`.

Needs [Tailscale](https://tailscale.com) with its CLI on every machine.

Flags: `--port` (default 2015) for the viewer and agent, and `--interval`
(100–60000 ms, default 1000) for `magi agent install`. Keys: `↑`/`↓` or `j`/`k`,
`g`/`G`, `Enter` to open an agent's Rex tab, `i` to message it, `y`/`n` to
allow or deny what it asks, `m` to move it, `a` for alerts, `q` to quit.

`magi fleet --json` prints every machine's load, each session's CPU and
memory, and your plan windows once, for scripts and tools that pick where to
run work.

## Performance

Agent on a MacBook (Apple M4), measured with `bench/agent_load.py`:

| Load | CPU | Memory | Worst 1% delay |
|---|---|---|---|
| Idle, nobody watching | 0.0% | 5.9 MiB | – |
| 32 viewers at 10 updates/s, all cores busy, 1,000 extra processes | 0.73% | 6.9 MiB | 105 ms |
| 32 viewers at 10 updates/s, reading 5 live Claude sessions | 2.9% | 12.3 MiB | 104 ms |

## FAQ

**Why no central server?** Each agent answers for its own machine, so there's
nothing to host or keep in sync.

**Does the status line slow Claude Code down?** No. It prints first and gives
the agent at most 300 ms. It only edits `statusLine` in
`~/.claude/settings.json`, never replaces one you already have, and keeps a
backup.

**Can magi tell when Codex needs approval?** Not yet: Codex doesn't write
approval prompts to its session files, so its sessions show working or idle,
and you answer its prompts in its pane. You can still message a Codex session
in Rex once it's idle.

**How accurate is a session's CPU?** It counts the session and every process
it started that's alive when magi looks, once a second. Very short ones, such
as each compiler run under `cargo`, can finish in between, so a busy build may
read a little low.

**What if Claude Code changes its session files?** That folder isn't a
documented format. magi notices, says so, and falls back to transcript activity
for the affected sessions.

## Roadmap

- Sessions magi starts and runs itself, so you can message and approve any
  session, not only those in Rex.
- More agents: Gemini CLI, Cursor, Aider, OpenCode.
- Alert thresholds you can set.

## Security

What magi reads, opens and stores is in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
