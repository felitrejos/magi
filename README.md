<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Move a live Claude Code session to another machine and back, with the conversation and your uncommitted work.</b>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="A Claude Code session on a Mac runs a small server on localhost:3000. The user types hand this off to my homelab. When the turn ends, the session moves to the homelab with the server's uncommitted edit, and the Mac's localhost:3000 now answers from the homelab. Claude there remembers the change and says it is uncommitted. Told to hand this back to the Mac, the session returns." width="820">
</p>

You install the plugin and ask Claude. There are no magi commands to type.
magi talks to your machines only over your own SSH, between macOS and
Linux.

## Install

Run these in Claude Code on your laptop.

```text
/plugin marketplace add felitrejos/magi
/plugin install magi@magi
```

Then say "set up magi with my homelab". Claude says what it will install
and waits for your yes. Each machine needs `curl`, git 2.31 or newer,
`tar`, tmux and Claude Code signed in with a claude.ai account. Your
laptop needs SSH to the other machine without a password prompt.

## What you can ask

- **"Hand this off to my homelab."** When the turn ends, the session
  moves with its conversation, branch, uncommitted and untracked work,
  secret files and Claude's project memory. It continues in a tmux window
  there, and in the Claude app through Remote Control. Claude asks first
  when secret files, untracked files, or 50 MB or more would go. A
  handoff that fails leaves the session running where it was.
- **"Hand this back to my Mac."** Say it in the session on the homelab.
- **"Move session X here"** or **"move it to the server."** Claude moves
  another session in either direction, unless it is busy or waiting on
  you.
- **"Does anything need me?"** Claude lists the sessions on all your
  machines and names the ones waiting on you.
- **"What did magi do?"** Claude lists moves that were cut off, recent
  handoffs, parked checkouts and forwarded ports.
- **"Let this project's setup run."** A project's `.magi.toml` can name a
  `setup` command, `services` and `ports` to forward back. Nothing in it
  runs until you accept its exact commands in a dialog.
- **"Check my homelab"** runs setup's checks again. **"Uninstall magi"**
  removes the watcher and the downloaded program.

Nothing that is running moves.

## Send work

<p align="center">
  <img src="assets/background-work.gif" alt="In a Claude Code session on a Mac, the user asks for a background Claude to build a linux/amd64 Docker image on a machine that runs amd64 natively, and meanwhile to add a /stats route on the Mac. Claude proposes the homelab and the user agrees. The Mac session writes /stats while a background Claude on the homelab writes a Dockerfile and builds the image. When it finishes, the Dockerfile and .dockerignore land in the Mac's working tree, and git status shows the uncommitted changes." width="820">
</p>

Ask Claude to run a shell command, a background Claude or a new session on
another machine. Claude proposes a machine and waits for your yes. The
work runs in its own copy of your working tree, in a tmux window you can
watch. A background Claude's changes come back into your working tree.

In `~/.config/magi/config.toml`, `max_agents`, `min_free_memory` and
`quiet_hours` under a host stop magi from starting new work there past
them. A top-level `exclude` names untracked files and folders that stay
behind, such as `node_modules`.

## Docs

- [Tutorial](docs/tutorial.md). Five minutes from one machine to two.
- [Troubleshooting](docs/troubleshooting.md). Each message and what to do.
- [Support](docs/support.md). Which machines work, how they are tested,
  measured times and what a version promises.
- [SECURITY.md](SECURITY.md) and [PRIVACY.md](PRIVACY.md). Everything
  magi runs, copies and writes.
- [ROADMAP.md](ROADMAP.md) and the [changelog](CHANGELOG.md).

Licensed [MIT](LICENSE).
