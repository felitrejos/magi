# Roadmap

magi is built around [Rex](https://www.superlogical.com/rex): every session it
drives lives in a Rex tab, and Rex is how magi sees, types into and opens
them. This is what's left, in the order it's planned.

## Phase 3: every session in Rex

Done:
- `c` starts a Claude Code or Codex session in any project on any machine, in
  its project's Rex tab, and magi selects it.
- `x` stops a running turn.

Next:
- **Adopt a desktop session.** Continue a Claude desktop session in a Rex tab
  on the same machine, stopping the desktop copy, so magi can message,
  approve and move it.
- **Multi-line messages and pastes**, sent as a bracketed paste with Rex's
  `block.event` paste event rather than typed line by line.
- **Messages while it works.** Queue them the way Claude Code does, safely,
  so they can't land in a prompt that opens meanwhile.
- **Permission mode.** Switch a session between ask, accept edits, plan and
  auto from magi.

## Phase 4: polish, revolving around Rex

What Rex offers that magi doesn't use yet, most valuable first:

1. **Events instead of polling.** One `rex events -s SESSION --json` stream
   per server replaces the `block ls` refresh and the 500 ms screen capture:
   magi wakes only on `title_changed`, `process_changed`, `pwd_changed`,
   `program_status_changed`, `bell`, `desktop_notification` and
   `child_exited`.
2. **Faster state and alerts.** Claude's title spinner, its bell and its
   desktop notifications as immediate signals for working and blocked.
3. **Find panes through Rex.** `rex block ls -A --json` with
   `rex block call -s S -b B process` (foreground pid, argv, cwd, last exit)
   instead of reading process environments, which also works on other hosts.
4. **Keys and pastes the Rex way.** `rex send-key escape` / `enter`, and the
   paste event for prompts.
5. **Floating panes for outputs.** A plan, a diff (`git diff | delta`) or an
   image beside the session: `rex split --json` then
   `rex api call session.float_block`, minimized or closed with
   `session.set_layer_state` and `session.close_layer`.
6. **Rex port forwarding.** Swap cielago's own SSH tunnels for
   `rex forward new` / `open`, and show `rex forward ls --json` states
   (active, away, orphaned) in magi.
7. **Exits without polling.** `rex wait --json` or `child_exited`.
8. **A folder picker from Rex.** `list_dir` to browse any machine's folders
   when starting a session, beyond the projects cielago knows.
9. **Land on the agent.** After opening a session's tab, `rex zoom --on` its
   pane, and label agent panes with `session.set_block_label`.
10. **Later:** stream screens over Rex's data connection instead of
    capturing, and attach images with `upload_begin`.

Rex is in preview, so magi should check its API revision
(`rex version --json`) and keep working when a call it relies on changes.

Other polish:
- **Machine detail:** top processes by CPU, memory and disk, and temperature.
- **Sessions:** PR status, worktree and branch labels, and subagent output.
- **Codex:** see its approval prompts.
- **Forwards:** clean up a forward that points at a machine the session moved
  on from, and handle dev servers listening on IPv6 localhost.

## Before 0.6.0

- A dry run of the release workflow, which now builds magi and cielago into
  one archive, and of `install.sh` installing both.
- Release notes, and magi and cielago versioned together.
