<p align="center">
  <img src="assets/cielago.jpeg" alt="cielago" width="180">
</p>

<h1 align="center">cielago</h1>

<p align="center">
  <b>Your session, on another machine, in seconds.</b><br>
  Tell Claude Code "hand this off to my homelab" and a new session there picks up the same conversation, with your branch, your changes and your <code>.env</code>, right in your Rex window.
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#what-moves">What moves</a> ·
  <a href="#configure">Configure</a> ·
  <a href="#questions">Questions</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/go-1.27-00ADD8" alt="Go 1.27">
  <img src="https://img.shields.io/badge/Claude%20Code-plugin-D97757" alt="Claude Code plugin">
  <img src="https://img.shields.io/badge/macOS%20·%20Linux-black" alt="macOS and Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="The Rex app. On the Mac's cielago tab, Claude says what the project does, is asked to hand itself off to the homelab, and arms the handoff. The Rex app then switches to the homelab's cielago · claude tab, where the same session confirms it's on the homelab, answers a follow-up from the conversation that began on the Mac, and takes the next instruction." width="820">
</p>
<p align="center">
  <i>A real handoff in the Rex app: the session leaves the Mac's <code>cielago</code> tab and keeps going in the homelab's.</i>
</p>

Some sessions run for hours: a big refactor, a migration, an agent working
through a backlog. A laptop is the wrong place for them, because it sleeps the
moment you close the lid. Until now, moving one to an always-on machine meant
starting over and explaining everything again.

cielago moves the session itself. Ask for it in plain words and, as soon as the
current turn ends, your project, your uncommitted work, your secret files,
Claude's memory of the project and the whole conversation land on the other
machine. A new session starts there in its own Rex tab and picks up exactly
where Claude was, your Rex app jumps straight to it, and the copy on your laptop
closes. The new session also runs with Remote Control, so when you're away from
Rex you can carry on from any Claude app on your account: desktop, phone or web.

## What you get

1. **Seconds, not a fresh start.** A handoff takes a few seconds once the turn
   ends, most of it Claude starting up on the other side.
2. **The whole conversation.** The session is resumed from its own transcript,
   not a summary, so everything Claude knew on your laptop it still knows.
3. **Your working tree, exactly.** Commits, the branch, staged and unstaged
   changes, new, deleted and binary files, matched to the same repository on
   the other machine even when it lives at a different path.
4. **Your secrets and Claude's memory.** `.env` files travel over SSH, never
   through the conversation, and the project's memory is merged with what the
   other machine already remembers.
5. **It opens where you are.** The session gets its own tab in
   [Rex](https://superlogical.com) and your Rex app switches to it. If it ran
   in a Rex pane on your laptop, that pane closes.
6. **Any direction.** Laptop to server, server to laptop, Mac to Mac, or on to
   a third machine. When a session comes back to a checkout it left, cielago
   knows nothing changed there and picks up where it stopped.
7. **One place at a time.** It never overwrites work made on the other machine,
   undoes anything it created if a handoff fails, and retires the old copy so
   the same session never runs twice.

```text
inspect 0.3s · sync 0.0s · transcript 0.2s · launch 0.9s · start 1.2s · tab 0.7s   →   3.4s
```
<p align="center"><i>Handing off a project the server had never seen, as cielago reports it.</i></p>

## How it works

The `handoff` skill tells the agent to stop anything running in the
background, run `cielago arm`, then end its turn. When the turn is over, the
plugin's Stop hook does the move:

1. Finds the project on the server: the same repository by its `origin` remote,
   or by its first commit if it has none, and creates it in the matching group
   folder if the server doesn't have it.
2. Snapshots the working tree and the staging area as two git commits, pushes
   them with your branch, and recreates both exactly on the server. It refuses
   if the server has work of its own or commits you don't have.
3. Copies the gitignored files matching `secrets`, the transcript, and Claude's
   project memory.
4. Starts a new session that resumes the conversation in its own Rex tab, with
   Remote Control on and a first message telling Claude where it is now and
   what was running before.
5. Switches every Rex app connected to the server to that tab, waits until the
   session is up, then closes the laptop's copy and marks its checkout as
   handed off.

| Part | What it does here |
|---|---|
| **Rex** | Where the session lives on each machine, organized by project, and the window you keep working in. |
| **Remote Control** | Puts the same session in every Claude app on your account, for when you're away from Rex. |
| **SSH** | Carries the commits, files, secrets, memory and transcript. |
| **Your network** | Anything that lets `ssh <server>` and Rex reach the other machine. Tailscale works well; nothing depends on it. |

## What moves

| | |
|---|---|
| **Moves** | The conversation; commits and branch; staged and unstaged changes; untracked files; files matching `secrets`; Claude's project memory; subagent transcripts. |
| **Restarts** | Background shells, dev servers and background subagents. The agent stops them before the handoff and writes down what each was doing; the new session reads that note and restarts what still needs to run. |
| **Stays** | Dependencies and build output such as `node_modules`; your user-level `~/.claude` settings and plugins, which each machine keeps its own. |

**Projects the server doesn't have yet** are created where they'd be on your
laptop. With `local_roots = ["~/Projects"]` and the server's
`roots = ["~/developer"]`:

| On your laptop | On the server | Rex session · tabs |
|---|---|---|
| `~/Projects/acme/api` | `~/developer/acme/api` | `acme` · `api`, `api · claude` |
| `~/Projects/notes` | `~/developer/notes/notes` | `notes` · `notes`, `notes · claude` |

The project tab runs `new_tab_command`, for example `claude rc`, so the project
is ready for Remote Control like your others. The first time Claude runs in a
new folder it asks you to trust it; answer once in the Rex tab and the handoff
carries on.

**Folders that aren't git repositories** are copied whole, minus anything in
`exclude`, to the same mirrored location.

## Install

On every machine you hand off from:

```bash
go install github.com/felitrejos/cielago@latest
claude plugin marketplace add felitrejos/cielago
claude plugin install cielago@cielago
```

Every machine needs [Rex](https://superlogical.com), with **Remote Control**
turned on in the Rex app's settings so cielago can switch it to the new session,
plus `git`, `tar`, and Claude Code logged in with `claude auth login` (Remote
Control doesn't work with `setup-token` tokens). The machine you hand off to
must accept SSH from the one you hand off from.

## Configure

`~/.config/cielago/config.toml`, on each machine you hand off from:

```toml
local_roots = ["~/Projects"]                    # where your projects live on this machine
exclude = ["node_modules", ".venv", "dist"]     # skipped when copying folders that aren't git repos

[hosts.homelab]
ssh = "homelab"                                 # ssh destination
claude = "~/.local/bin/claude"                  # claude binary there
roots = ["~/developer"]                         # where projects live there; new ones go in the first
secrets = [".env", ".env.*"]                    # gitignored files to bring along
rex_server = "homelab"                          # its Rex host
new_tab_command = "claude rc --name {repo} --spawn same-dir"  # optional: runs in a new project's tab
```

The name of the table is what you call the machine: "hand this off to my
homelab" finds `[hosts.homelab]`.

## Questions

**Can I pick it up somewhere other than Rex?**
Yes. The new session runs with Remote Control, so it's in the Claude app on your
laptop and phone and on claude.ai, under your account.

**Can I hand it back, or on to another machine?**
Yes, from wherever it runs now, as long as that machine has cielago set up and
can SSH to the next one. When it comes back to a checkout it left, cielago sees
that nothing changed there and updates it. If you edited that checkout in the
meantime, it refuses rather than overwrite your work.

**What about subagents and background jobs?**
Nothing that's running can move. Before the handoff the agent stops background
shells, dev servers and background subagents and writes a short note about each.
The new session gets the note and the earlier subagents' transcripts, and
restarts what's still needed.

**Does it work from the Claude desktop app?**
Yes. A session in the app tells you where it went and archives itself, so only
the new one is left. The app asks you to approve the archive with one click.

**What if something goes wrong halfway?**
The session keeps running where it was and Claude tells you what failed. A
folder or Rex tab cielago created for the handoff is removed again; if it had
already updated an existing checkout, the message says so.

**Where do my secrets go?**
From one machine's disk to the other's, over SSH. They never enter the
conversation, the transcript or cielago's output. The sending machine's copy
replaces the other's.

## Limits

- It moves between turns, never in the middle of a reply or a tool call.
- A git repository needs at least one commit.
- In folders that aren't git repositories, files deleted on one machine stay on
  the other.
- Changes inside git submodules don't move.
- macOS and Linux only.

## License

[MIT](LICENSE)
