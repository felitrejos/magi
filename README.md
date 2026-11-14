<p align="center">
  <img src="assets/cielago.jpeg" alt="cielago" width="80">
</p>

<h1 align="center">cielago</h1>

<p align="center">
  <b>Your session, on another machine, in seconds.</b><br>
  Tell Claude Code "hand this off to my homelab" and a new session there picks up the same conversation, with your branch, your changes and your <code>.env</code>.<br>
  Or keep the session here and send it work there: a slow test suite, a task for a background Claude, a second session.
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#what-moves">What moves</a> ·
  <a href="#send-work-to-another-machine">Send work elsewhere</a> ·
  <a href="#commands">Commands</a> ·
  <a href="#configure">Configure</a> ·
  <a href="#questions">Questions</a>
</p>

<p align="center">
  <a href="https://github.com/felitrejos/cielago/actions/workflows/ci.yml"><img src="https://github.com/felitrejos/cielago/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/Claude%20Code-plugin-D97757" alt="Claude Code plugin">
  <img src="https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows%20soon-black" alt="macOS and Linux, Windows soon">
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
| **Moves** | The conversation; commits and branch; staged and unstaged changes; untracked files; changes inside submodules; uncommitted Git LFS files; files matching `secrets`; Claude's project memory; subagent transcripts. A repository without any commits yet moves too. |
| **Restarts** | Background shells, dev servers and background subagents. The agent stops them before the handoff and writes down what each was doing; the new session reads that note and restarts what still needs to run. A project can also say what to run on arrival, in [`.cielago.toml`](#set-up-the-project-on-arrival). |
| **Stays** | Dependencies and build output such as `node_modules`; your user-level `~/.claude` settings and plugins, which each machine keeps its own. |

**Projects the other machine doesn't have yet** are created where they'd be on
your laptop. With `local_roots = ["~/Projects"]` and the server's
`roots = ["~/developer"]`:

| On your laptop | On the server | Session · tab |
|---|---|---|
| `~/Projects/acme/api` | `~/developer/acme/api` | `acme` · `api · claude` |
| `~/Projects/notes` | `~/developer/notes/notes` | `notes` · `notes · claude` |

The first time Claude runs in a new folder it asks you to trust it; answer
once in the new tab and the handoff carries on.

**Folders that aren't git repositories** are mirrored, minus anything in
`exclude`: files you deleted on one side are deleted on the other too. cielago
remembers what each copy looked like when a session left it, and refuses to
mirror over a copy that someone changed since.

### Set up the project on arrival

Dependencies don't move, and dev servers stop. Put a `.cielago.toml` at the
root of the project to say what to run when it arrives on the other machine:

```toml
setup = "pnpm install"           # runs once, in a pane below Claude
services = ["pnpm dev"]          # each in its own pane, once setup succeeds
```

The panes open in the session's Rex or tmux tab, beside Claude, which is
told what's starting there and where setup reports how it went. A handoff
doesn't wait for them. Commands run in your login shell. `cielago pull` runs
the setup in your terminal before Claude continues there, and closes the
session's tab on the other machine, panes included.

Because these commands come from the repository, cielago runs them only after
you approve that file, in that project, with `cielago allow`. If the file
changes, the next handoff skips it and says so until you allow it again.

## Send work to another machine

A handoff moves the whole session. Often you only want to send one piece of
work: run the test suite on the homelab while you keep going, or give a side
task to a Claude there. The plugin gives every Claude Code session, in the
terminal, in Rex and in the desktop app, tools for that:

| Tool | What it does |
|---|---|
| `machines` | Lists your machines with their live load from [magi](https://github.com/felitrejos/magi), whether they answer over SSH, and what's running there. |
| `run_elsewhere` | Runs a shell command there, such as `go test ./...`, and returns its exit code and the end of its output. |
| `delegate` | Gives a self-contained task to a background Claude there, like a subagent on another machine. You get its final message, and its changes as a branch. |
| `start_session` | Opens a new interactive session for the project there, in a tab of its own with Remote Control. |
| `task_status` · `task_result` · `cancel_task` | Checks on a task, collects it, or stops it. |

Ask in plain words, "run the tests on whichever machine is lighter", and
Claude checks `machines`, proposes a machine and what it will run there, and
lets you choose: there, here instead, or not at all. Claude Code then asks you to approve the tool call itself,
as for any tool.

Every task is a copy, never a move:

- **Your exact working tree.** Commits, staged and unstaged changes, untracked
  files and secret files go to a checkout of the task's own under
  `~/.cielago/tasks` on the other machine, filled from your clone there when
  it has one. Your checkout there is never touched, and the session you asked
  from keeps running.
- **Out of your way.** The start tool copies the project, usually in a few
  seconds, and returns a task id; the work runs in the background, so a long
  test run never blocks Claude or times out. Claude checks back with
  `task_status`.
- **In plain sight.** Each task runs in its own tab in the project's Rex
  session, or tmux window, without taking focus, so you can watch it.
- **Results come back.** `task_result` returns a command's exit code with the
  last 80 lines of output, and copies the full log to `~/.cielago/tasks` here.
  For a delegated Claude it returns its final message, and fetches its changes
  into your repository as the branch `cielago/task-<id>`, next to your working
  tree rather than into it. Collecting a task closes its tab and removes its
  copy.
- **Dependencies.** `node_modules` and the like aren't copied; the project's
  approved [`.cielago.toml`](#set-up-the-project-on-arrival) setup runs first.

**Which machine.** Your hosts are tried in the order they appear in the
config, and the first one with room gets the work. A host has room when it's
outside its quiet hours, its budget allows another agent and enough memory is
free, and it isn't over 90% busy, judged from magi's live numbers when
`magi fleet` reports it. When the task starts, the host checks its own budget
once more, so two tasks started together can't both take its last slot; a
host that turns out to be full is skipped for the next, and the result says
why. cielago matches magi's machine names to your hosts by name or by their
`ssh` destination. Without magi, it takes the first host that answers, and
says so. `machines` and `cielago doctor` show each host's budget and whether
it has room.

**What a delegated Claude may do.** It runs headless with
`claude -p --permission-mode acceptEdits --permission-prompts none`: it reads
and edits files in its own copy, and anything else that would ask for
permission, such as shell commands, network access or files outside its copy,
is denied, because nobody is there to answer. To let it run your tests, Claude
passes rules such as `Bash(go test:*)` in `allowed_tools`, which you see
before you approve. Rules in that machine's Claude Code settings apply too.
`permission_mode` can also be `dontAsk`, for only what's allowed, or `auto`,
Claude Code's auto mode. It never runs with permission checks bypassed.

`cielago tasks` lists tasks from the terminal; `cielago tasks show <id>`,
`cancel <id>` and `clean` do what the tools do.

## Install

```bash
go install github.com/felitrejos/cielago@latest
claude plugin marketplace add felitrejos/cielago
claude plugin install cielago@cielago
cielago init
```

Prebuilt binaries for macOS and Linux are on the
[releases page](https://github.com/felitrejos/cielago/releases). On macOS, a
binary downloaded with a browser is quarantined; clear that once with
`xattr -d com.apple.quarantine cielago`. Claude Code runs `cielago` by name,
so put it in a folder on your `PATH` (`go install` uses `~/go/bin`).

Do this on every machine you hand off from. Each machine needs git 2.31 or
newer, `tar`, and Claude Code logged in with `claude auth login` (Remote
Control doesn't work with `setup-token` tokens), plus
[Rex](https://superlogical.com/rex) or tmux. The other machine's login shell
doesn't matter (bash, zsh, fish and csh all work), and BusyBox systems such as
Alpine work too.
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
| `cielago move <session> <host>` | Moves a session running on this machine from outside it: at once if it's idle, or when its turn ends. `--json` prints each step and the outcome as JSON lines, for programs such as [magi](https://github.com/felitrejos/magi). |
| `cielago status` | Recent handoffs, and checkouts here that a session left. |
| `cielago unpark [dir]` | Stops treating a checkout as one a session left. |
| `cielago allow [dir]` | Lets the project's `.cielago.toml` run after handoffs, until the file changes. |
| *"run the tests on the homelab"* | Said to Claude: runs them there in a copy of your working tree; see [Send work elsewhere](#send-work-to-another-machine). |
| `cielago tasks` | Work sent to other machines and its state. |
| `cielago tasks show <id>` | A task's output; collects it if it's finished. |
| `cielago tasks cancel <id>` | Stops a task, closes its tab and removes its copy. |
| `cielago tasks clean` | Collects every finished task. |

## Configure

`cielago init` writes `~/.config/cielago/config.toml`:

```toml
local_roots = ["~/Projects"]                    # where your projects live on this machine
exclude = ["node_modules", ".venv", "dist"]     # folder names skipped when copying and when looking for secrets

[hosts.homelab]
ssh = "homelab"                                 # ssh destination
claude = "~/.local/bin/claude"                  # claude binary there
roots = ["~/developer"]                         # where projects live there; new ones go in the first
secrets = [".env", ".env.*"]                    # gitignored files to bring along, from any folder
launcher = "rex"                                # "rex" or "tmux"
rex_server = "homelab"                          # its Rex host, with launcher = "rex"
new_tab_command = "claude rc --name {repo} --spawn same-dir"  # optional: runs in a new project tab
max_agents = 3                                  # optional: no new delegated Claude or session past 3 working agents
min_free_memory = "4G"                          # optional: no new task with less memory free than this
quiet_hours = "22:00-07:00"                     # optional: no new tasks in this window, in this machine's time
```

The budget fields only limit tasks sent there; handoffs ignore them.
`max_agents` counts Claude Code sessions in the middle of a turn there, and
cielago's delegated Claudes and sessions; a `run_elsewhere` command isn't an
agent, so only memory and quiet hours apply to it.

The name of the table is what you call the machine: "hand this off to my
homelab" finds `[hosts.homelab]`. The order of the tables is your preference
when cielago picks a machine for a task.

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

**Will a delegated Claude ask me to trust its folder?**
No: headless Claude skips that. A session opened with `start_session` does ask,
since every task gets a new folder. Run `claude` once in `~/.cielago/tasks` on
that machine and trust it, and later sessions there don't ask.

**Which Claude Code versions work?**
cielago reads some Claude Code internals, such as the transcript format.
`cielago doctor` warns when a machine runs a version newer than cielago was
tested with.

## License

[MIT](LICENSE)
