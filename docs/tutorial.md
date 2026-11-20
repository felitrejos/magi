# Tutorial

This takes about five minutes. You start with Claude Code on one machine
and end with a session that moves to a second machine and back. The
examples call the first machine your Mac and the second one `homelab`. Use
your own names.

You never type a magi command. You install the plugin once, and after that
you ask Claude.

## Before you start

- Two machines that run macOS or Linux. [support.md](support.md) lists
  which pairs work.
- `ssh homelab` from your Mac logs in without a password prompt.
- Both machines have git 2.31 or newer, `tar`, `curl`, tmux and Claude
  Code, logged in with a claude.ai account.

## 1. Install the plugin

Run these in Claude Code on your Mac.

```text
/plugin marketplace add felitrejos/magi
/plugin install magi@magi
```

The first time the plugin runs, its launcher downloads the magi program
that matches the plugin's version into `~/.magi/bin` and checks its
SHA-256 against the release's checksums.

## 2. Set up the second machine

Say this to Claude.

```text
set up magi with my homelab
```

Claude tells you what it will install and waits for your yes. Then magi
does these in order.

- It checks SSH to `homelab` and finds Claude Code there.
- It saves the machine in `~/.config/magi/config.toml`.
- It installs the plugin on `homelab`.
- It starts a background watcher on your Mac, so sessions can be handed
  back to it.
- It checks git, `tar`, tmux, the Claude Code version and the login on
  both machines.
- It moves a throwaway session to `homelab` and back, then removes it.
  That takes a minute or two and three short Claude turns on your account.

When everything passes, the result ends with a line that starts like
this.

```text
magi works with homelab: the session went there and back with its uncommitted change, and still remembered OTTER.
```

If a check fails, Claude says what to fix. Some fixes are yours alone,
such as accepting a host key or logging in to Claude Code there.
[troubleshooting.md](troubleshooting.md) covers each message.

## 3. Hand a session off

Open Claude Code in a project on your Mac, do some work, and leave an
edit uncommitted. Then say this.

```text
hand this off to my homelab
```

Claude stops anything it has running in the background, because nothing
that is running moves. Then it shows you what goes across. That is the
branch, its commits, your changed files, new files git doesn't track yet,
secret files by name, the size, and the conversation.

Claude asks you before it copies when secret files, untracked files, or
50 MB or more would go. Say yes to go on.

The move starts when Claude's turn ends. On the owner's machines it takes
about 3 seconds. The result names each step and its time, where the
session runs, and its Remote Control link.

```text
Brought session f62c66e4 back from homelab to /Users/you/Developer/Personal/magi-e2e in 6.1s (inspect 1.5s, stop 2.0s, recheck 0.2s, sync 0.5s, transcript 0.4s, launch 0.1s, start 1.4s). It's running in tmux session magi-e2e on your-mac, and in the Claude apps at https://claude.ai/code/...
```

That is a real hand back from the owner's server to their Mac. Your step times will differ.

The session now runs in a tmux window on `homelab`. Open the link to
continue it in the Claude app, or attach to tmux on `homelab`. The copy on
your Mac closes and refuses new messages.

## 4. Hand it back

In the session on `homelab`, say this.

```text
hand this back to my Mac
```

Your Mac doesn't need to accept SSH. The watcher on your Mac sees the
request and pulls the session over its own connection when the turn ends.
The session opens in a tmux window on your Mac with Remote Control, and
your uncommitted edit is back in the checkout you left.

## 5. Move another session

You can move a session other than the one you are in. Start by asking
what is running.

```text
does anything need me?
```

Claude lists the sessions on all your machines and names the ones waiting
on you. Then ask for a move in either direction.

```text
move the billing session to the homelab
```

```text
bring the billing session here
```

Claude says which session goes where and waits for your yes. A session
that is in the middle of a turn, or waiting on you, isn't moved. Claude
waits for it to go idle, or asks you to answer it first. A session you
quit moves too, when you give its full id.

## 6. Approve a project's setup

A project can say what to run after it arrives. Put a `.magi.toml` in the
project's root.

```toml
setup = "pnpm install"
services = ["pnpm dev"]
ports = [3000]
```

Nothing in the file runs until you approve it. Say this.

```text
let this project's setup run
```

Claude Code shows a dialog with the file's path, the machine and the
exact commands and ports. Claude can't answer that dialog for you. Your
approval holds for that exact file. Any edit to it needs a new yes.

After that, each move runs `setup` and each service in its own tmux pane
beside the session on the machine it arrives on. After a handoff,
`localhost:3000` on the machine you left reaches the session's server
until the session comes back.

## What to ask next

- "What did magi do?" lists moves that were cut off, recent handoffs,
  parked checkouts and forwarded ports.
- "Check my homelab" runs the checks and the round trip again.
- "Run the tests on my homelab" sends work there and brings the result
  back. Claude proposes the machine and waits for your yes.
- "Uninstall magi" removes the watcher and `~/.magi/bin` and keeps your
  config and records.
