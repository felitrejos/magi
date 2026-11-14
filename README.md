<p align="center">
  <img src="assets/cielago.jpeg" alt="cielago" width="80">
</p>

<h1 align="center">cielago</h1>

<p align="center">
  <b>Your session, on another machine, in seconds.</b><br>
  Tell Claude Code "hand this off to my homelab" and a new session there picks up the same conversation, with your branch, your changes and your <code>.env</code>.
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#what-moves">What moves</a> ·
  <a href="#commands">Commands</a> ·
  <a href="#configure">Configure</a> ·
  <a href="#questions">Questions</a>
</p>

<p align="center">
  <a href="https://github.com/felitrejos/cielago/actions/workflows/ci.yml"><img src="https://github.com/felitrejos/cielago/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/Claude%20Code-plugin-D97757" alt="Claude Code plugin">
  <img src="https://img.shields.io/badge/macOS%20·%20Linux-black" alt="macOS and Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="Rex on a MacBook. In the Mac's acme session, Claude says what the project does, is asked to hand itself off to the homelab, and arms the handoff. The view then switches to the homelab's acme session, whose only tab, api · claude, is the same session: it confirms it's on the homelab and answers a follow-up from the conversation that began on the Mac." width="820">
</p>
<p align="center">
  <i>A real handoff in <a href="https://superlogical.com/rex">Rex</a>: the session leaves the Mac and opens on the homelab in a tab of its own.</i>
</p>

Some sessions run for hours: a big refactor, a migration, an agent working
through a backlog. A laptop is the wrong place for them, because it sleeps the
moment you close the lid. Until now, moving one to an always-on machine meant
starting over and explaining everything again.

cielago moves the session itself. Ask for it in plain words and, as soon as the
current turn ends, your project, your uncommitted work, your secret files,
Claude's memory of the project and the whole conversation land on the other
machine. A new session starts there in a tab of its own and picks up exactly
where Claude was, and the copy on your laptop closes. With
[Rex](https://superlogical.com/rex), your window jumps straight to it. The new
session also runs with Remote Control, so you can carry on from any Claude app
on your account: desktop, phone or web.

## What you get

1. **Seconds, not a fresh start.** A handoff takes a few seconds once the turn
   ends, most of it Claude starting up on the other side.
2. **The whole conversation.** The session is resumed from its own transcript,
   not a summary, so everything Claude knew it still knows.
3. **Your working tree, exactly.** Commits, the branch, staged and unstaged
   changes, new, deleted and binary files, matched to the same repository on
   the other machine even when it lives at a different path.
4. **Your secrets and Claude's memory.** `.env` files travel over SSH, never
   through the conversation, and the project's memory is merged with what the
   other machine already remembers.
5. **It opens where you are.** The session gets its own tab in
   [Rex](https://superlogical.com/rex), and Rex switches to it, or its own
   window in tmux.
6. **Any direction.** Laptop to server, Mac to Mac, on to a third machine, and
   back again with `cielago pull`, even when your laptop doesn't accept SSH.
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

1. Finds the project on the other machine: the same repository by its `origin`
   remote, or by its first commit if it has none, and creates it in the
   matching group folder if it isn't there yet.
2. Snapshots the working tree and the staging area as two git commits, sends
   them with your branch, and recreates both exactly. It refuses if the other
   checkout has work of its own or commits you don't have.
3. Copies the gitignored files matching `secrets`, the transcript, and Claude's
   project memory.
4. Starts a new session that resumes the conversation in a tab of its own, with
   Remote Control on and a first message telling Claude where it is now and
   what was running before.
5. Brings that tab to the front in [Rex](https://superlogical.com/rex), waits
   until the session is up, then closes the old copy and marks its checkout as
   handed off.

`cielago pull <host>` runs the same steps the other way, from the machine that
wants the session back: it waits for the session there to finish its turn,
stops it, brings everything back, and continues it in your terminal.

| Part | What it does here |
|---|---|
| **[Rex](https://superlogical.com/rex)** or **tmux** | Where the session lives on each machine, organized by project, and the window you keep working in. |
| **Remote Control** | Puts the same session in every Claude app on your account. |
| **SSH** | Carries the commits, files, secrets, memory and transcript. |
| **Your network** | Anything that lets `ssh <host>` reach the other machine. Tailscale works well; nothing depends on it. |

## What moves

| | |
|---|---|
| **Moves** | The conversation; commits and branch; staged and unstaged changes; untracked files; files matching `secrets`; Claude's project memory; subagent transcripts. |
| **Restarts** | Background shells, dev servers and background subagents. The agent stops them before the handoff and writes down what each was doing; the new session reads that note and restarts what still needs to run. |
| **Stays** | Dependencies and build output such as `node_modules`; your user-level `~/.claude` settings and plugins, which each machine keeps its own. |

**Projects the other machine doesn't have yet** are created where they'd be on
your laptop. With `local_roots = ["~/Projects"]` and the server's
`roots = ["~/developer"]`:

| On your laptop | On the server | Session · tab |
|---|---|---|
| `~/Projects/acme/api` | `~/developer/acme/api` | `acme` · `api · claude` |
| `~/Projects/notes` | `~/developer/notes/notes` | `notes` · `notes · claude` |

The first time Claude runs in a new folder it asks you to trust it; answer
once in the new tab and the handoff carries on. **Folders that aren't git
repositories** are copied whole, minus anything in `exclude`.

## Install

```bash
go install github.com/felitrejos/cielago@latest
claude plugin marketplace add felitrejos/cielago
claude plugin install cielago@cielago
cielago init
```

Prebuilt binaries for macOS and Linux are on the
[releases page](https://github.com/felitrejos/cielago/releases).

Do this on every machine you hand off from. Each machine needs `git`, `tar`,
and Claude Code logged in with `claude auth login` (Remote Control doesn't work
with `setup-token` tokens), plus [Rex](https://superlogical.com/rex) or tmux.
With [Rex](https://superlogical.com/rex), turn on **Remote Control** in its settings so cielago can switch it
to the new session. The machine you hand off to must accept SSH from the one
you hand off from; to bring a session back to a laptop that doesn't, use
`cielago pull`.

## Commands

| | |
|---|---|
| *"hand this off to my homelab"* | Said to Claude: moves the session there. |
| `cielago init` | Writes the config for this machine and a machine to hand off to, then checks it. |
| `cielago doctor [host]` | Checks ssh, tools, Claude Code's version and login, roots and [Rex](https://superlogical.com/rex) or tmux on both ends, and says how to fix anything missing. |
| `cielago pull <host> [session]` | Brings a session back from `<host>` and continues it here. Without a session id, it takes the last one you handed off there. |
| `cielago status` | Recent handoffs, and checkouts here that a session left. |
| `cielago unpark [dir]` | Stops treating a checkout as one a session left. |

## Configure

`cielago init` writes `~/.config/cielago/config.toml`:

```toml
local_roots = ["~/Projects"]                    # where your projects live on this machine
exclude = ["node_modules", ".venv", "dist"]     # skipped when copying folders that aren't git repos

[hosts.homelab]
ssh = "homelab"                                 # ssh destination
claude = "~/.local/bin/claude"                  # claude binary there
roots = ["~/developer"]                         # where projects live there; new ones go in the first
secrets = [".env", ".env.*"]                    # gitignored files to bring along
launcher = "rex"                                # "rex" or "tmux"
rex_server = "homelab"                          # its Rex host, with launcher = "rex"
new_tab_command = "claude rc --name {repo} --spawn same-dir"  # optional: runs in a new project tab
```

The name of the table is what you call the machine: "hand this off to my
homelab" finds `[hosts.homelab]`.

## Questions

**Can I pick it up somewhere other than [Rex](https://superlogical.com/rex)?**
Yes. The new session runs with Remote Control, so it's in the Claude app on your
laptop and phone and on claude.ai, under your account. With tmux,
`ssh -t <host> tmux attach` gets you to it.

**How do I bring it back?**
Run `cielago pull <host>` on the machine you want it on. It waits for the
session to finish its turn, moves it back and continues it in your terminal.
Machines that can reach each other over SSH can also simply hand off again.

**What if I edited the checkout a session left?**
A checkout a session left is marked as handed off. When the session comes back
and nothing changed there, cielago updates it. If you edited it meanwhile, it
refuses rather than overwrite your work; commit or stash, or run
`cielago unpark`.

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
folder or tab cielago created for the handoff is removed again, and trying
again picks up from where it stopped.

**Where do my secrets go?**
From one machine's disk to the other's, over SSH, never through the
conversation. See [SECURITY.md](SECURITY.md).

**Which Claude Code versions work?**
cielago reads some Claude Code internals, such as the transcript format.
`cielago doctor` warns when a machine runs a version newer than cielago was
tested with.

## Limits

- It moves between turns, never in the middle of a reply or a tool call.
- A git repository needs at least one commit.
- In folders that aren't git repositories, files deleted on one machine stay on
  the other.
- Git LFS files and changes inside git submodules don't move.
- macOS and Linux only.

## License

[MIT](LICENSE)
