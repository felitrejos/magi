# Privacy

Last updated 18 November 2026.

magi is software you run on your own machines. I run no servers for it and
collect nothing. There is no telemetry, no analytics, no accounts and no
crash reports.

## What magi copies, and where

The `magi` program connects only to machines you configured yourself, over
SSH, with your own SSH keys and config. The Claude Code plugin only runs
`magi`, so the same goes for it.

To move a session or send work, magi copies three things to the machine you
chose.

- The Claude Code or Codex session transcript.
- The project's git working tree, including uncommitted changes.
- Any secrets files the project lists in its `.magi.toml`.

It doesn't send them anywhere else. [SECURITY.md](SECURITY.md) has the
details of what a handoff copies.

## Installing

The install script downloads the release from GitHub
([github.com/felitrejos/magi](https://github.com/felitrejos/magi) releases).
That is GitHub's normal download and nothing extra.

## The overview

`magi-overview` is optional and for Rex users. It does three more things.

- It runs a small agent on each of your machines, and those agents talk to
  your other machines over your Tailscale network.
- It reads your Claude plan usage from Anthropic's API (`api.anthropic.com`)
  with the Claude Code sign-in already on that machine, only to show your
  limits.
- It runs the GitHub CLI (`gh`) with your existing login to show pull
  request status.

Nothing from these is sent to me or to anyone else.

## Other services

Claude Code, Codex, Anthropic, OpenAI, GitHub, Tailscale and Rex have their
own privacy policies for what they do. magi doesn't change those.

## Questions

Open an issue on
[github.com/felitrejos/magi](https://github.com/felitrejos/magi/issues).
