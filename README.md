<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Run your coding agents across all your machines, from one Rex pane.</b><br>
  Every Claude Code and Codex session on your tailnet, what each one costs and uses, a way to talk to them, and a way to move them to whichever machine has room.
</p>

<p align="center">
  <a href="#move-work-between-machines"><b>Move work</b></a> ·
  <a href="#install">Install</a> ·
  <a href="#in-rex-and-other-terminals">Rex</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#faq">FAQ</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-2024-CE422B" alt="Rust 2024">
  <img src="https://img.shields.io/badge/Rex-pane-111111" alt="Rex pane">
  <img src="https://img.shields.io/badge/Claude%20Code%20·%20Codex-sessions-D97757" alt="Claude Code and Codex sessions">
  <img src="https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows%20soon-black" alt="macOS and Linux, Windows soon">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <img src="assets/fleet.gif" alt="magi in the Rex app: machines and agents on the left; on the right a home server's CPU, memory and network as pixel charts, then the conversations of Claude Code and Codex sessions drawn as Markdown, and the alerts view." width="820">
</p>
<p align="center">
  <i>A Mac and a home server, six agent sessions, one waiting on you.</i>
</p>

Agents now run on several machines at once, and each can stall on a prompt,
fill its context or quietly spend. magi is a multiplexer for them: one view of
every session and every machine's load, and the means to act on it. Answer a
session, message it, or move it, with its conversation and uncommitted work,
to the machine that has room.

## Highlights

- **Move work to where there's room.** When one machine is busy and another
  idle, magi suggests moving the heavy session, and `m` moves it, conversation,
  branch and uncommitted work included, to continue there in seconds. Claude
  can also send tests and side tasks to the lightest machine.
- **Every session, every machine.** Each Mac and Linux box on your tailnet
  running a magi agent shows up on its own, with its load. Sessions are
  grouped by project, related repos together (`stampr` and `stampr-ops`), with
  the ones waiting on you on top and their branch and worktree beside them.
  No server or config file.
- **Start, read and answer.** `c` opens a launcher over magi, as a Rex
  floating pane: pick a project, the machine (magi suggests the one with the
  most room), Claude Code or Codex and its model, and it starts in that project's Rex tab
  and magi selects it. Select a session to read
  its conversation, typeset like a page on a Mac (real type, highlighted code,
  tables) and drawn as Markdown elsewhere; `↵` writes to it,
  `y` or `n` answers its permission prompt, `1`–`9` picks an option when it
  asks a question, and `x` stops a turn. A session in the Claude desktop app
  or another terminal continues in Rex with `r`, as a copy.
- **Limits and alerts.** Your Claude and Codex 5-hour and weekly windows, and
  alerts when a session needs you, a machine goes quiet or a plan nearly runs
  out.
- **Light and private.** The agent idles at 0% CPU, and only devices signed in
  to your tailnet as you can connect.

## Move work between machines

This is what sets magi apart from a dashboard. It ships with **cielago**, a
second command that does the moving: magi shows where there's room, cielago
acts on it.

- **Move a session.** Press `m` in magi, or tell Claude "hand this off to my
  homelab". When the turn ends, the conversation, your branch and uncommitted
  changes, the `secrets` files you list and Claude's memory land on the other
  machine, and the session continues there in a tab of its own with Remote
  Control. magi follows it there, and no prompt waits for you on the other
  side. `cielago pull <host>` brings one back.
- **Send work.** Every Claude Code session gets tools to run a command
  elsewhere (`run_elsewhere`), hand a task to a background Claude
  (`delegate`) or open a second session (`start_session`). Each runs in a copy
  of your working tree on the lightest machine within its budget, and Claude
  asks you first.

<p align="center">
  <img src="assets/handoff.gif" alt="magi in the Rex app: a Claude Code session on a MacBook is selected, m and y move it to the home server, the footer shows each step, and magi lands on the same session on the home server, its conversation intact, in about four seconds." width="820">
</p>

`cielago init` writes `~/.config/cielago/config.toml` with your machines, where
projects live on each, and which files count as secrets; `cielago doctor`
checks both ends.

A project's `.cielago.toml` says what to start when it arrives, and which
ports should follow it:

```toml
setup = "pnpm install"
services = ["pnpm dev"]
ports = [3000]
```

After a move, `localhost:3000` on the machine it left reaches the dev server
on the new one, over SSH, so your browser tab keeps working. magi shows the
port in the session's header, and moving the session back closes the forward.
Approve the file once with `cielago allow`.

## Install

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
magi agent install    # on every machine
magi                  # to look, best in a Rex pane on an always-on machine
```

The script installs `magi` and `cielago` to `~/.local/bin` for macOS or Linux,
x86_64 or arm64, after checking their SHA-256. Every machine needs
[Tailscale](https://tailscale.com). Two optional steps:

- `magi statusline install` where you use Claude Code, for its context use and
  plan windows.
- To move sessions and work between machines, on each one:
  `cielago init`, then
  `claude plugin marketplace add felitrejos/magi && claude plugin install cielago@magi`.

Keys: `↑↓` select, `c` new session, `↵` or a click on the conversation to
write (`⇧↵` for a new line), `y`/`n` allow or deny, `1`–`9` choose, `x` stop,
`r` continue in Rex, `o` open in Rex, `m` move, `d` its changes and `v` its
plan in a floating pane over magi, `p` move its repo to another project, `P`
rename its project (onto another name to merge them), `a` alerts, `q` quit.
`magi fleet --json` prints the same picture once, for scripts.

## In Rex and other terminals

magi is made to stay open in [Rex](https://www.superlogical.com/rex): one pane
on an always-on machine that you open from the Mac, iOS or the web, with
alerts on Rex's session picker and tab headers. `↵` opens a session's Rex tab
on whichever machine.

Other terminals work too. Charts are pixel-sharp where the kitty graphics
protocol is supported (Rex, Ghostty, kitty 0.28+) and drawn with block
characters elsewhere. Colours follow your terminal's theme, light or dark.

## How it works

Each machine runs `magi agent` in the background (launchd or a systemd user
service). The viewer finds agents with `tailscale status` and gets a compact
sample from each every second. The agent only reads what the coding agents
already write:

| Source | Gives magi |
|---|---|
| `~/.claude/sessions/`, `~/.claude/projects/` | Claude Code sessions, their state, model, tokens and cost |
| `magi statusline` | Claude's context use and plan windows |
| `~/.codex/sessions/` | Codex sessions with the same, plus plan windows |

Cost is priced from token counts at list prices, the same figure Claude Code's
`/cost` shows. When you message a session or answer its prompt, the agent on
that machine types it into the session's own Rex pane.

| Agent on an M4 MacBook | CPU | Memory |
|---|---|---|
| Nobody watching | 0% | 6 MB |
| 32 viewers, every core busy | 0.7% | 7 MB |
| 32 viewers, 5 live Claude sessions | 2.9% | 12 MB |

## FAQ

**Can magi tell when Codex needs approval?** Not yet. Codex doesn't write its
approval prompts to disk, so answer those in its pane.

**Does the status line slow Claude Code down?** No. It prints first and gives
the agent at most 300 ms, and it never replaces a status line you already have.

**What if Claude Code changes its files?** They aren't a documented format.
magi notices, says so, and falls back to what the transcripts show.

## Roadmap

Next: multi-line messages, and using more of Rex, such as its event stream and
floating panes. The full list
is in [ROADMAP.md](ROADMAP.md).

What magi and cielago read, send and change is in [SECURITY.md](SECURITY.md).
Licensed [MIT](LICENSE).
