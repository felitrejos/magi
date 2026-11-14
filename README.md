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
  <a href="#with-magi">With magi</a> ·
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
through a backlog. A laptop sleeps the moment you close the lid, and moving a
session to an always-on machine used to mean starting over.

cielago moves the session itself. Ask in plain words and, when the current
turn ends, the project, your uncommitted work, your secret files, Claude's
memory and the whole conversation land on the other machine. A new session
picks up there in a tab of its own, with Remote Control so it's in every
Claude app on your account, and the copy on your laptop closes.

## What you get

1. **Seconds, not a fresh start.** Most of a handoff is Claude starting up on
   the other side.
2. **The whole conversation,** resumed from its own transcript, not a summary.
3. **Your working tree, exactly:** commits, branch, staged and unstaged
   changes, matched to the same repository even at a different path.
4. **Your secrets and Claude's memory,** over SSH, never through the
   conversation.
5. **It opens where you are:** its own tab in [Rex](https://superlogical.com/rex),
   which switches to it, or in tmux.
6. **Any direction,** and back again with `cielago pull`.
7. **One place at a time.** It never overwrites work on the other machine,
   undoes what it created if it fails, and retires the old copy.

```text
inspect 0.3s · sync 0.0s · transcript 0.2s · launch 0.9s · start 1.2s · tab 0.7s   →   3.4s
```
<p align="center"><i>Handing off a project the server had never seen, as cielago reports it.</i></p>

## How it works

The `handoff` skill has the agent stop anything running in the background,
run `cielago arm` and end its turn. Then the plugin's Stop hook:

1. Finds the project on the other machine by its `origin` remote, or its first
   commit, and creates it if it isn't there.
2. Sends the working tree and staging area as two snapshot commits with your
   branch, and recreates both. It refuses if that checkout has work of its own.
3. Copies the `secrets` files, the transcript and the project's memory.
4. Resumes the conversation there in a new tab with Remote Control, and tells
   Claude where it is and what was running.
5. Waits until it's up, closes the old copy and marks its checkout as handed
   off.

`cielago pull <host>` does the same the other way, from the machine that
wants the session back.

| Part | What it does here |
|---|---|
| **[Rex](https://superlogical.com/rex)** or **tmux** | Where sessions live on each machine, by project. |
| **Remote Control** | Puts the session in every Claude app on your account. |
| **SSH** | Carries everything. Any network that lets `ssh <host>` work will do; Tailscale works well. |

## What moves

| | |
|---|---|
| **Moves** | The conversation; commits and branch; staged, unstaged and untracked changes, submodules and Git LFS files included; files matching `secrets`; Claude's project memory; subagent transcripts. |
| **Restarts** | Background shells, dev servers and subagents. The agent notes what each was doing and the new session restarts what's needed; [`.cielago.toml`](#set-up-the-project-on-arrival) can do it for you. |
| **Stays** | Dependencies such as `node_modules`, and your user-level `~/.claude` settings. |

A project the other machine doesn't have yet is created where it would be on
your laptop. With `local_roots = ["~/Projects"]` and the server's
`roots = ["~/developer"]`:

| On your laptop | On the server | Session · tab |
|---|---|---|
| `~/Projects/acme/api` | `~/developer/acme/api` | `acme` · `api · claude` |
| `~/Projects/notes` | `~/developer/notes/notes` | `notes` · `notes · claude` |

Folders that aren't git repositories are mirrored, deletions included, and a
copy someone changed since a session left it is never overwritten.

### Set up the project on arrival

A `.cielago.toml` at the project's root says what to run when it arrives:

```toml
setup = "pnpm install"           # runs once, in a pane below Claude
services = ["pnpm dev"]          # each in its own pane, once setup succeeds
```

Claude is told what's starting beside it. Because the commands come from the
repository, they only run once you approve that file with `cielago allow`, and
again after every change to it.

## Send work to another machine

Sometimes you only want to send one piece of work: the test suite, or a side
task for another Claude. The plugin gives every Claude Code session tools for
that:

| Tool | What it does |
|---|---|
| `machines` | Your machines, their live load from [magi](https://github.com/felitrejos/magi), and what's running there. |
| `run_elsewhere` | Runs a command, such as `go test ./...`, and returns its exit code and output. |
| `delegate` | Gives a task to a background Claude there; you get its final message and its changes as a branch. |
| `start_session` | Opens a new interactive session for the project there. |
| `task_reply` | Sends a follow-up to a delegated Claude, which keeps its context. |
| `task_status` · `task_result` · `cancel_task` | Checks on a task, collects it, or stops it. |

Ask in plain words, "run the tests on whichever machine is lighter", and
Claude proposes a machine and what it will run, and you choose: there, here
instead, or not at all.

- **A copy, never a move.** Each task gets your exact working tree in a
  checkout of its own under `~/.cielago/tasks` there. Your checkout there and
  the session you asked from are untouched.
- **In the background, in plain sight.** It runs in its own Rex or tmux tab,
  and Claude checks back with `task_status`, so a long run never blocks.
- **Results come back.** A command's exit code, the end of its output and the
  full log; a delegated Claude's message, with its changes fetched as the
  branch `cielago/task-<id>` beside your working tree. Its copy stays for
  follow-ups until `cancel_task` or a day idle.
- **Which machine.** The first host in your config with room: within its
  [budget](#configure) and not over 90% busy, judged from magi's live numbers.
  The host re-checks its budget as the task starts. Without magi, the first
  host that answers.
- **Safe defaults.** A delegated Claude runs with `acceptEdits`: it edits its
  copy, and anything else that would ask for permission is denied unless you
  approve rules such as `Bash(go test:*)`. It never runs with permission checks
  bypassed. See [SECURITY.md](SECURITY.md).
- **Your plan.** Every machine shares your Claude plan, so when magi reports a
  plan window 90% used, Claude work doesn't start unless you say so.

`cielago tasks` lists tasks from the terminal.

## With magi

[magi](https://github.com/felitrejos/magi) shows your machines and their
agents live in one Rex pane: where there's room, and what each session costs.
cielago acts on it. Press magi's handoff key on a session and `cielago move`
sends it to the machine you pick; when Claude sends work elsewhere, cielago
reads `magi fleet` for room, load and plan usage. Each works without the other.

## Install

```bash
go install github.com/felitrejos/cielago@latest
claude plugin marketplace add felitrejos/cielago
claude plugin install cielago@cielago
cielago init
```

Prebuilt binaries are on the
[releases page](https://github.com/felitrejos/cielago/releases); on macOS,
clear a browser download's quarantine with
`xattr -d com.apple.quarantine cielago`. Put `cielago` on your `PATH`.

Do this on every machine you hand off from. Each needs git 2.31 or newer,
`tar`, Claude Code logged in with `claude auth login`, and
[Rex](https://superlogical.com/rex) (with **Remote Control** on in its
settings) or tmux. The machine you hand off to must accept SSH; to bring a
session back to one that doesn't, use `cielago pull`.

## Commands

| | |
|---|---|
| *"hand this off to my homelab"* | Said to Claude: moves the session there. |
| *"run the tests on the homelab"* | Said to Claude: runs them there in a copy of your working tree. |
| `cielago init` | Writes the config and checks it. |
| `cielago doctor [host]` | Checks both ends and says how to fix anything missing. |
| `cielago pull <host> [session]` | Brings a session back and continues it here. |
| `cielago move <session> <host>` | Moves a session on this machine from outside it, now or when its turn ends. `--json` reports each step. |
| `cielago status` | Recent handoffs, and checkouts here a session left. |
| `cielago unpark [dir]` | Stops treating a checkout as one a session left. |
| `cielago allow [dir]` | Lets the project's `.cielago.toml` run, until it changes. |
| `cielago tasks` | Lists tasks; `show <id>`, `cancel <id>` and `clean` collect or stop them. |

## Configure

`cielago init` writes `~/.config/cielago/config.toml`:

```toml
local_roots = ["~/Projects"]                    # where your projects live on this machine
exclude = ["node_modules", ".venv", "dist"]     # folder names never copied

[hosts.homelab]
ssh = "homelab"                                 # ssh destination
claude = "~/.local/bin/claude"                  # claude binary there
roots = ["~/developer"]                         # where projects live there; new ones go in the first
secrets = [".env", ".env.*"]                    # gitignored files to bring along
launcher = "rex"                                # "rex" or "tmux"
rex_server = "homelab"                          # its Rex host, with launcher = "rex"
new_tab_command = "claude rc --name {repo} --spawn same-dir"  # optional: runs in a new project tab
max_agents = 3                                  # optional budget for tasks: working agents,
min_free_memory = "4G"                          #   free memory,
quiet_hours = "22:00-07:00"                     #   and no new tasks in this window
```

The table's name is what you call the machine, and the tables' order is your
preference when cielago picks one for a task. Budgets only limit tasks, not
handoffs.

## Questions

**Can I pick it up somewhere other than [Rex](https://superlogical.com/rex)?**
Yes. With Remote Control it's in the Claude app on your laptop and phone and
on claude.ai. With tmux, `ssh -t <host> tmux attach`.

**What if I edited the checkout a session left?**
cielago refuses to overwrite it; commit or stash, or run `cielago unpark`.

**Does it work from the Claude desktop app?**
Yes. The session there tells you where it went and archives itself.

**What if something goes wrong halfway?**
The session keeps running where it was, cielago removes what it created, and
trying again picks up from where it stopped.

**Why does a new session ask me to trust its folder?**
Claude asks once per new folder. For `start_session` tasks, run `claude` once
in `~/.cielago/tasks` on that machine and trust it.

**Which Claude Code versions work?**
cielago reads some Claude Code internals; `cielago doctor` warns when a
machine runs a newer version than cielago was tested with.

## License

[MIT](LICENSE)
