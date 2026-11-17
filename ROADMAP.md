# Roadmap

## Direction

- **An overview, not a workspace.** You work in Claude Code, Codex and the
  Claude app. magi shows every session on every machine, tells you what
  needs you, and starts new sessions in the right place.
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
- Re-record the README's demos as videos, since they show the old interface.

## Decided against

- **Live screens through Rex's `data.connect`.** It would need a persistent
  connection and a terminal emulator inside magi, for what the agents' own
  panes already show.
- **Attaching images from magi.** Dropping an image on the session's own Rex
  pane already uploads it to that machine.
