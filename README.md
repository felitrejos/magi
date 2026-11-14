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
  <a href="#new-projects">New projects</a> ·
  <a href="#configure">Configure</a> ·
  <a href="#questions">Questions</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/go-1.27-00ADD8" alt="Go 1.27">
  <img src="https://img.shields.io/badge/Claude%20Code-plugin-D97757" alt="Claude Code plugin">
  <img src="https://img.shields.io/badge/Rex-required-black" alt="Rex required">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="The Rex app. On the Mac's cielago tab, Claude says what the project does, is asked to hand itself off to the homelab, and arms the handoff. The Rex app then switches to the homelab's cielago tab: Remote Control running on the left, and on the right the same Claude session, which confirms it's on the homelab, answers a follow-up from the conversation that began on the Mac, and takes the next instruction." width="820">
</p>
<p align="center">
  <i>A real handoff in the Rex app: the session leaves the Mac's <code>cielago</code> tab and keeps going in the homelab's.</i>
</p>

Some sessions run for hours: a big refactor, a migration, an agent working
through a backlog. A laptop is the wrong place for them, because it sleeps the
moment you close the lid. Until now, moving one to an always-on machine meant
starting over and explaining everything again.

cielago moves the session itself. Ask for it in plain words and, as soon as the
current turn ends, your branch, your uncommitted work, your secret files and the
whole conversation land on the other machine. A new session starts there in the
repository's Rex tab and picks up exactly where Claude was, your Rex app jumps
straight to it, and the copy on your laptop closes. The new session also runs
with Remote Control, so when you're away from Rex you can carry on from any
Claude app on your account: desktop, phone or web.

## What you get

1. **Seconds, not a fresh start.** A handoff takes a few seconds once the turn
   ends, most of it Claude starting up on the other side.
2. **The whole conversation.** The session is resumed from its own transcript,
   not a summary, so everything Claude knew on your laptop it still knows.
3. **Your working tree comes along.** Commits, the current branch, edited, new
   and deleted files, all matched to the same repository on the other machine
   even when it lives at a different path.
4. **Your secrets too.** `.env` files and anything else you list travel over
   SSH, straight from disk to disk, never through the conversation.
5. **It opens where you are.** The session runs in a pane beside the
   repository's tab in [Rex](https://superlogical.com), and your Rex app
   switches to it, so you can keep typing as if nothing moved. If it ran in a
   Rex pane on your laptop, that pane closes.
6. **New projects land organized.** A project the server has never seen gets a
   clone in the same group folder it has on your laptop, a Rex session for the
   group and a tab for the repository.
7. **One place at a time.** It refuses to clobber work on the other machine,
   undoes anything it created if a handoff fails, and retires the old copy so
   the same session never runs twice.

```text
inspect 0.7s · clone 0.0s · sync 0.1s · transcript 0.2s · launch 1.9s · start 1.3s · tab 0.3s   →   4.5s
```
<p align="center"><i>Handing off a project the server had never seen, as cielago reports it.</i></p>

## How it works

The `handoff` skill tells the agent to run `cielago arm`, then end its turn.
When the turn is over, the plugin's Stop hook does the move:

- finds the same repository on the server by its `origin` remote, or by its
  first commit if it has none, and creates a clone if the server doesn't have
  one;
- pushes your commits and checks out your branch, and stops if the server has
  uncommitted work or commits you don't have;
- copies changed, new and deleted files, plus files matching `secrets`;
- copies the transcript into the server's `~/.claude/projects`;
- starts a new session that resumes the conversation, in a pane beside the
  repository's Rex tab, with Remote Control on and a first message telling
  Claude where it is now;
- switches every Rex app connected to the server to that pane;
- waits until the session is up, then closes the laptop's copy and renames its
  transcript so it can't be resumed twice.

Each part has one job:

| Part | What it does here |
|---|---|
| **Rex** | Where the session lives on the server, organized by project, and the window you keep working in. |
| **Remote Control** | Puts the same session in every Claude app on your account, for when you're away from Rex. |
| **SSH** | Carries the commits, files, secrets and transcript. |
| **Your network** | Anything that lets `ssh <server>` and Rex reach the server. Tailscale works well; nothing depends on it. |

## New projects

When the server doesn't have a project yet, cielago mirrors where it lives on
your laptop. With `local_roots = ["~/Developer/Personal"]` and the server's
`roots = ["~/developer"]`:

| On your laptop | On the server | Rex session · tab |
|---|---|---|
| `~/Developer/Personal/stampr/stampr-ops` | `~/developer/stampr/stampr-ops` | `stampr` · `stampr-ops` |
| `~/Developer/Personal/kodo` | `~/developer/kodo/kodo` | `kodo` · `kodo` |

The new tab's first pane runs `new_tab_command`, for example
`claude rc --name {repo} --spawn same-dir`, so the project is ready for Remote Control like the
rest. The first time Claude runs in the new folder it asks you to trust it;
answer once, right there in the Rex tab, and the handoff carries on.

## Install

```bash
go install github.com/felitrejos/cielago@latest
claude plugin marketplace add felitrejos/cielago
claude plugin install cielago@cielago
```

You need [Rex](https://superlogical.com) on your laptop, connected to the
server, with **Remote Control** turned on in the Rex app's settings so cielago
can switch it to the new session. On the server, cielago needs `git`, `tar`,
and Claude Code logged in with `claude auth login`. Remote Control doesn't work
with `setup-token` tokens.

## Getting started

1. Make sure `ssh <server>` works with a key, no password prompt.
2. Write `~/.config/cielago/config.toml` (see below).
3. In any Claude Code session, say **"hand this off to my homelab"**.

## Configure

`~/.config/cielago/config.toml`:

```toml
local_roots = ["~/Developer/Personal"]          # where your projects live on this laptop

[hosts.homelab]
ssh = "homelab"                                 # ssh destination
claude = "~/.local/bin/claude"                  # claude binary on the server
roots = ["~/developer"]                         # where clones live there; new ones go in the first
secrets = [".env", ".env.*", ".dev.vars"]       # gitignored files to bring along
rex_server = "homelab"                          # the server's Rex host
new_tab_command = "claude rc --name {repo} --spawn same-dir"  # optional: runs in a new project's tab
```

The name of the table is what you call the machine: "hand this off to my
homelab" finds `[hosts.homelab]`.

## Questions

**Can I pick it up somewhere other than Rex?**
Yes. The new session runs with Remote Control, so it's in the Claude app on
your laptop and phone and on claude.ai, under your account.

**Does it work from the Claude desktop app?**
Yes. A session in the app tells you where it went and archives itself, so only
the new one is left. The app asks you to approve the archive with one click.

**What happens to dev servers and background tasks?**
They stay behind. The conversation and the files move; processes start fresh
on the server when you need them.

**Can I hand it back to the laptop?**
Not yet. That needs the server to reach your laptop over SSH, which most home
setups don't allow.

**What if something goes wrong halfway?**
The session keeps running on your laptop and Claude tells you what failed. A
clone or Rex tab cielago created for the handoff is removed again; if it had
changed an existing checkout on the server, the message says so.

**Where do my secrets go?**
From your laptop's disk to the server's, over SSH. They never enter the
conversation, the transcript or cielago's output. The laptop's copy replaces
the server's, so values that only exist in the server's `.env` are overwritten.

## Limits

- It moves between turns, never in the middle of a reply or a tool call.
- A project needs at least one commit.
- Staged and unstaged changes both arrive unstaged.
- Claude's per-project memory folder stays separate on each machine.
- `node_modules` and other build output aren't copied; reinstall if a lockfile
  changed.

## License

[MIT](LICENSE)
