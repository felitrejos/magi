# Roadmap

## Direction

- **Two layers, one tool.** Moving sessions and sending work is the base,
  and needs only SSH. The overview is the layer on top, for Rex and
  Tailscale. The base never depends on the overview.
- **An overview, not a workspace.** You work in Claude Code, Codex and the
  Claude app. magi shows every session on every machine, tells you what
  needs you, and suggests where work should run.
- **Leave it to the tools.** What Claude Code, Codex or Rex already do stays
  there. The overview keeps only what needs the whole fleet, which is
  answering prompts, diffs, moves, starts, limits and alerts.
- **Suggest, don't act.** magi suggests moves and machines but never moves
  or starts anything by itself. The one exception is closing a port forward
  left pointing at a machine its session has moved on from.
- **A layer for other tools.** The CLI and `magi mcp` give scripts, other
  orchestrators and Claude itself the same actions.
- **Follow Rex.** While Rex is in preview, magi supports only its latest
  release. Bump `REX_API` and `rexAPI` with each one.

## Decided against

- **Live screens through Rex's `data.connect`.** It would need a persistent
  connection and a terminal emulator inside magi, for what the agents' own
  panes already show.
- **Attaching images from magi.** Dropping an image on the session's own Rex
  pane already uploads it to that machine.
