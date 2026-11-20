# Changelog

What changed in each version of magi. Past versions are listed as far as
the git history shows them. [docs/versioning.md](docs/versioning.md) says
what a version number promises.

## Unreleased

The plugin's version in this work is 0.10.0. No release with that number
is published.

### Agent native

- You install magi by adding the plugin in Claude Code. The plugin's
  launcher downloads the release binary that matches its version into
  `~/.magi/bin` and checks its SHA-256. The install script is gone.
- Nobody types magi commands. `init`, `try`, `doctor`, `move`, `pull`,
  `status` and the other typed commands are gone. Claude runs
  `magi arm`, `magi wait` and `magi tasks wait` itself, and everything
  else is a tool.
- New tools replace the commands. `setup_machine` checks SSH, installs the
  plugin on the other machine, starts the watcher, checks both machines
  and runs a throwaway round trip. `move_session` moves another session in
  either direction, a quit one included. `status`, `unpark`,
  `allow_project` and `uninstall` cover the rest.
- You approve a project's `.magi.toml` in a dialog Claude Code shows. The
  approval holds for that file's SHA-256 only.
- A delegated Claude that magi starts doesn't get the tools that are your
  call, nor `status`. It keeps `machines`, `sessions`, `diff` and
  `task_status`.
- Every message tells Claude what to do next. Host names and SSH
  destinations are checked before magi connects.

### Removed

- Codex support. magi moves Claude Code sessions only. The Codex session
  listing, moves, watching, the per-host `codex` key and the setup option
  are gone. An old config with a `codex` key still loads, and the key is
  ignored. `remote/tested.json` keeps a frozen `codex` range only so that
  released 0.9 installs can still parse the file.
- Rex. tmux is the only launcher, and port forwards always use `ssh`.
- The `magi-overview` program and everything only it used.

### Before a copy

- Before a move, magi lists what goes across. The list has the branch and
  its changed files, files git doesn't track yet by name, secret files by
  name only, Git LFS files, the size of the changed and new files, and
  what the project's `.magi.toml` runs.
- Claude asks you first when secret files, untracked files, or 50 MB or
  more would go. `magi arm` says "Not armed" and arms nothing until it
  runs again with `--yes`, on the hand-back path too. `move_session`
  returns the list and moves nothing until it is called with `confirmed`.
- `exclude` now applies to untracked files in a git repository. Untracked
  files and folders with those names stay behind, and the list names what
  was left out. Tracked files under those names still go.
- The default `exclude` (`node_modules`, `.venv`, `dist`, `build`,
  `target`) and `secrets` (`.env`, `.env.*`) apply when the config has
  none. `setup_machine` no longer writes them into the config.

### Moves

- Push and pull run through one pipeline. A pull now carries folder trust
  and runs the project's setup and services in tmux panes, as a push does.
- A move's result gives the Remote Control link and the time each step
  took.
- A session can be handed back to a laptop that takes no incoming SSH.
  `magi watch` on the laptop keeps one outgoing connection to each machine
  and takes back sessions that ask for it.
- magi refuses to move a session when Claude Code on either machine is a
  version its tests haven't passed with. It reads the list from
  `remote/tested.json` on `main` and keeps a copy for an hour.
- `magi wait` returns once a session isn't busy and says whether it is
  idle, waiting on you or ended.
- `sessions` opens with the sessions waiting on you across your machines.
- When SSH dies in the middle of a move, the error says the host "stopped
  answering".
- `setup_machine` refuses WSL on either machine.

### Recovery

- Every move writes a journal in `~/.magi/moves` before it changes
  anything and removes it as its last step.
- magi settles a move that was cut off on its own. It checks the journals
  when a Claude Code session starts, once a minute from the watcher,
  before the next move of that session, and when you ask for `status`.
- Recovery works out which copy is live, closes the other, and never
  deletes a folder or a transcript. When both copies took new turns, it
  stops neither and leaves the choice to you.

### Watch

- `magi watch` streams what runs on each machine over the connection it
  already keeps, and writes the latest to `~/.magi/live/<machine>.json`.
  That is each machine's load, memory and disk, and each session's state,
  CPU and memory.
- Per-session CPU includes child processes that already exited on Linux.
  macOS doesn't expose that to other processes.
- Stopping the watcher marks every machine as not answering in
  `~/.magi/live`.
- A new watch loop stops orphaned old loops for its machine.
- The watcher restarts for a new build or a config change, once no
  session is being taken back.

### Testing and releases

- The nightly is a GitHub Actions workflow on Ubuntu and macOS runners. It
  runs the contract tests against the newest Claude Code and needs the
  repository secret `CLAUDE_CODE_OAUTH_TOKEN`. It raises the newest tested
  version when both runners pass and opens an issue that names the failed
  job otherwise. The list of failing versions is edited by hand.
- The nightly on the owner's server, its systemd timer and
  `ci/nightly.sh` are gone.
- A release publishes only after the CI jobs pass on macOS, Ubuntu and
  Alpine.
- Tests kill the hook at each step of a move and cut the connection.
- The real round trip between two machines is not in CI. It is a manual
  check with `setup_machine` before a release.

### Fixed

- Stopping a session could match the tmux server that started it, so a
  hand back of a project with a setup pane failed, and the stop could
  take the whole tmux server down.
- On macOS, stopping a session read a padded process id as empty and
  stopped nothing.
- A watch loop whose login shell didn't replace itself with the loop was
  left running as an orphan.
- A pull failed after the session had already moved when its Remote
  Control link was slow. It now succeeds and notes the missing link.
- `magi arm` armed the move even when it couldn't read what goes. It now
  returns the error and arms nothing.
- The `diff` tool failed for sessions on other machines.
- An edit of the same size made in the same second as the last git index
  write could be left out of a copy.
- A busy session never moved, because the move held the lock the Stop
  hook needed.
- Unpacking a copy could write through a symlinked folder or stop
  halfway. It now unpacks into a temporary folder first.
- A move is refused before it touches files when the branch is checked
  out in another git worktree.

## 0.9.1, 2026-11-18

- Hand back finds the machine you meant from part of its name.
- A delegated Claude keeps going after one denied command.
- The demos were recorded again.

## 0.9.0, 2026-11-18

- The Go program became `magi`, and the overview became the optional
  `magi-overview`.
- A session can be handed back from the machine it moved to.
- Handoffs in tmux work as well as the ones in Rex.
- A handoff holds up when something goes wrong partway.
- magi chooses machines, lists sessions and shows diffs over SSH.

## 0.8.0, 2026-11-18

- cielago became part of magi.
- A background Claude's changes are applied to your working tree when it
  comes back.

## 0.7.1, 2026-11-17

- Tested with Claude Code 2.1.326.
- Help text, tool descriptions and the plugin were brought in line with
  the code.

## 0.7.0, 2026-11-17

- Rex became the workspace and magi the overview, with one view made for
  Rex's sidebar.
- magi got a command-line client and an MCP server.
- Claude can send work to the machine that fits and is told when it is
  done.
- cielago, the tool that hands a session off, was released on its own as
  cielago 0.6.0 and 0.7.0.

## 0.5.0, 2026-11-15

- The viewer shows what each agent session and everything it started is
  using.
- The viewer suggests a session move and starts it through cielago.
- The selected agent's Rex pane shows live in the detail panel.

## 0.4.0, 2026-11-14

- `magi fleet` shows every machine's load and agents in one call.

## 0.3.0, 2026-11-14

- Enter on an agent opens the Rex tab it runs in.

## 0.2.0, 2026-11-14

- The viewer was redesigned around panels and pixel charts.
- Colours come from the terminal's theme and follow theme switches.

## 0.1.0, 2026-11-14

- First release. Live monitoring of your machines and coding agents in
  one Rex pane, with Codex sessions, alerts in Rex and release builds.
