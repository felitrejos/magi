# Roadmap

## Direction

- **Not a workspace.** You work in Claude Code, Codex and the Claude app.
  magi and cielago put that work on the right machine: they start
  sessions where there's room, move them, and send work to the machine
  that fits it. magi also shows every session on every machine and tells
  you what needs you.
- **Leave it to the tools.** What Claude Code, Codex or Rex already do stays
  there. magi keeps only what needs the whole fleet: answering prompts,
  diffs, moves, starts, limits and alerts.
- **Suggest, don't act.** magi suggests moves and machines but never moves
  or starts anything by itself. The one exception is closing a port forward
  left pointing at a machine its session has moved on from.
- **A layer for other tools.** The CLI and `magi mcp` give scripts, other
  orchestrators and Claude itself the same actions.
- **Follow Rex.** While Rex is in preview, magi and cielago support only its
  latest release. Bump `REX_API` and `rexAPI` with each one.

## Before 0.6.0

- A dry run of the release workflow, which builds magi and cielago into one
  archive, and of `install.sh` installing both.
- Release notes, with magi and cielago versioned together.

## Decided against

- **Live screens through Rex's `data.connect`.** It would need a persistent
  connection and a terminal emulator inside magi, for what the agents' own
  panes already show.
- **Attaching images from magi.** Dropping an image on the session's own Rex
  pane already uploads it to that machine.
