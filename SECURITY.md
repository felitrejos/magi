# Security

cielago moves a working session between machines you control. This is what
it touches and how.

## What moves, and how

- **Transport.** Everything travels over your own `ssh` connection to the other
  machine: git objects through `git push`/`git fetch`, files through `tar`
  streamed over the same connection. cielago runs no server and sends nothing
  anywhere else. Remote Control traffic is Claude Code's own and goes through
  your Anthropic account.
- **Secrets.** Only gitignored files matching your `secrets` patterns are
  copied, straight from one disk to the other. Their contents never enter the
  conversation, the transcript, cielago's output or its logs. The sending
  machine's copy replaces the receiving machine's.
- **Transcripts and memory.** The session's transcript and Claude's project
  memory are copied as files. They hold whatever the conversation held: if a
  secret was ever pasted into the chat, it moves with the transcript.
- **Credentials.** cielago never copies Claude Code's login, ssh keys or any
  credential store. Each machine logs in to Claude on its own.

## What it changes on the receiving machine

- It checks out your branch and recreates your uncommitted work in the
  project's checkout, but refuses if that checkout has work of its own, has
  commits you don't have, or has ignored files that would be overwritten.
- It creates a project folder only where none exists, and only copies a
  folder that isn't a git repository into a new folder or one it created
  before (listed in `~/.cielago/folders` on that machine).
- It writes the transcript and memory under `~/.claude/projects`, starts a
  session in a new Rex or tmux tab, and adds a `refs/cielago/...` ref while a
  handoff is in flight.

## Trust

The first time Claude runs in a new folder on a machine, Claude Code asks you
to trust it. cielago never answers that for you.

## Reporting a problem

Please report security issues privately through GitHub's
"Report a vulnerability" on this repository rather than in a public issue.
