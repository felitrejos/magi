# Changelog

## Unreleased

- Send work to another machine without moving the session. The plugin now
  ships a tool server with `machines`, `run_elsewhere`, `delegate`,
  `start_session`, `task_status`, `task_result` and `cancel_task`, available
  in every Claude Code session.
- Each task runs in a copy of your exact working tree, in a checkout of its
  own on the other machine, in a Rex or tmux tab you can watch. Your checkout
  there and the session that asked are never touched.
- Tasks start at once and run in the background: `run_elsewhere` returns the
  exit code, the end of the output and the full log; `delegate` returns the
  remote Claude's final message and its changes as the branch
  `cielago/task-<id>`.
- A delegated Claude edits only its own copy and runs anything else only
  through rules you approve; it never runs with permission checks bypassed.
- cielago picks the least loaded machine from `magi fleet`, or the first that
  answers when magi isn't there.
- `cielago tasks` lists tasks; `show`, `cancel` and `clean` collect or stop
  them. `cielago doctor` says whether magi reports machine load.
- A project's `.cielago.toml` says what to run when it arrives: `setup` runs
  in a pane below Claude, then each of `services` in its own pane. Claude is
  told they're running. `cielago pull` runs the setup before Claude continues.
- `cielago allow` shows a project's `.cielago.toml` and approves it for that
  project; changing the file needs a new approval.
- When a session leaves a machine, by `cielago pull` or another handoff, the
  tab cielago opened for it there closes, panes included.

## 0.3.0

- Committed Git LFS files arrive with their content even when their objects
  were never uploaded: cielago copies the LFS objects the other machine lacks.
- Secret files inside ignored folders come along; folders listed in `exclude`
  are skipped.
- A remote written with an SSH host alias (`git@github-work:acme/api`) matches
  the same repository written with the real host.
- Works when the other machine's login shell is fish, csh or tcsh, and on
  BusyBox systems such as Alpine.
- All shell code lives in scripts that shellcheck checks, and every path and
  name reaches them as an argument, never as part of the script.
- Secret files that are symlinks come along as symlinks, and folders cielago
  can't read no longer stop a handoff.
- A Claude session that doesn't exit within a minute of the handoff is killed,
  so its transcript is always retired.
- `cielago unpark` works during a merge or rebase.
- A new folder on the other machine is filled from the repository's `origin`
  first, so shallow clones and large histories hand off.
- Trying again after a failed handoff picks up from the copy it left, even if
  you kept working meanwhile.
- When the new session waits for you to trust its folder, the error says so.
- A session started inside a submodule hands off the whole project.
- The new session's start is timed by the other machine's clock, so clock
  drift between machines doesn't fail a handoff.
- tmux sessions cielago starts no longer inherit `LC_ALL=C`.
- `cielago pull <host>` says when the last session there was already brought
  back; `cielago unpark` also clears submodules.
- `cielago doctor` checks for git 2.31 or newer and gives each host its own
  time limit; `cielago init` keeps an existing tab command.
- Error messages show what went wrong without shell noise.
- Builds use the latest Go 1.27 patch release; CI runs staticcheck and
  govulncheck.

## 0.2.0

- Repositories without any commits can be handed off.
- Folders that aren't git repositories are mirrored, deletions included, and a
  copy changed since a session left it is never overwritten. A session can
  come back to the folder it started in.
- Uncommitted Git LFS files arrive with their real content.
- Uncommitted changes inside git submodules move too, and a submodule the
  other machine doesn't have yet is set up there with its own remote.
- Secret and Git LFS files are found across the whole repository when Claude
  runs in a subfolder.
- `exclude` takes file and folder names; a pattern with a slash is refused.

## 0.1.0

First release.

- Hand a running Claude Code session to another machine by asking for it in
  plain words; it continues there in a new session with Remote Control on.
- Moves the conversation, commits and branch, staged and unstaged changes,
  untracked files, secret files, Claude's project memory and subagent
  transcripts. Folders that aren't git repositories are copied whole.
- Background work is stopped first and described in a note the new session
  uses to restart it.
- Opens the session in its own tab in [Rex](https://superlogical.com/rex),
  switching Rex to it, or in tmux.
- `cielago pull` brings a session back from a machine that can't reach yours.
- Parks the checkout a session leaves, so it can come back to it, and refuses
  to overwrite work made anywhere else.
- `cielago init`, `doctor`, `status` and `unpark`.
- Works between macOS and Linux machines in any direction.
