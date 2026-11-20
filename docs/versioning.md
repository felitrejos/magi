# Versioning

magi uses `MAJOR.MINOR.PATCH` version numbers. The plugin's version in
`plugin/.claude-plugin/plugin.json` is the version of everything. The
launcher runs the magi program with the same number, and a release tag
must match it.

Until 1.0, any 0.x release can change anything on this page.
[CHANGELOG.md](../CHANGELOG.md) says what changed in each one.

## What 1.x won't break

From 1.0 on, a 1.x release keeps these working. A change that breaks one
of them needs 2.0.

### What you ask for

- The handoff skill. Saying "hand this off to" a machine moves the
  session you are in when the turn ends, and saying "hand this back"
  returns it.
- The rules that protect you. A failed move leaves the session running
  where it was. Claude asks you first when secret files, untracked files,
  or 50 MB or more would go. A project's `.magi.toml` runs nothing until
  you approve that exact file. Starting work on another machine, moving a
  session, setting a machine up and uninstalling wait for your yes.
- One live copy. After a move, the copy a session left refuses new
  messages.

### The plugin's tools

The tool names and what each one does stay. So do their existing inputs.

| Tool | Inputs that stay |
|---|---|
| `machines` | `needs` |
| `sessions` | none |
| `diff` | `session` |
| `move_session` | `session`, `to`, `confirmed` |
| `setup_machine` | `host`, `ssh`, `roots`, `local_roots` |
| `allow_project` | `dir`, `sha256` |
| `status` | none |
| `unpark` | `dir` |
| `run_elsewhere` | `command`, `host`, `dir`, `setup` |
| `delegate` | `task`, `host`, `dir`, `allowed_tools`, `permission_mode`, `model`, `setup`, `keep_branch` |
| `session_elsewhere` | `host`, `dir`, `prompt` |
| `task_status` | `task_id` |
| `task_result`, `cancel_task` | `task_id` |
| `task_reply` | `task_id`, `message` |
| `uninstall` | none |

A 1.x release can add a tool or an optional input. It won't remove or
rename one, and it won't make an optional input required.

A delegated Claude that magi starts keeps getting only `machines`,
`sessions`, `diff` and `task_status`. A 1.x release won't hand it a tool
that is your call.

### Config keys

These keys in `~/.config/magi/config.toml` keep their names and meaning.

| Key | Where |
|---|---|
| `local_roots`, `exclude` | top level |
| `ssh`, `claude`, `roots`, `secrets` | under `[hosts.<name>]` |
| `max_agents`, `min_free_memory`, `quiet_hours` | under `[hosts.<name>]` |

A config that works with one 1.x release loads in every later one. A key
magi no longer uses is ignored, not an error. The old `codex` key under a
host already loads that way.

A project's `.magi.toml` keeps its three keys, `setup`, `services` and
`ports`. An approval stays tied to the file's SHA-256.

### Files other programs read

Most of `~/.magi` is magi's own. Two things in it are for readers such as
the planned menu bar app, and they keep their shape.

- `~/.magi/live/<machine>.json`, written by `magi watch`. It keeps the
  fields `answering`, `heard`, `cores`, `load`, `memory` and `disk` with
  `usedMB` and `totalMB`, and `sessions`. Each session keeps `id`,
  `title`, `folder`, `state`, `waitingOn`, `since`, `cpu` and `memoryMB`.
  A 1.x release can add fields, so a reader must ignore the ones it
  doesn't know.
- `~/.magi/bin/magi`, the link to the current magi program, and
  `~/.magi/bin/magi-<version>` beside it.

### The tested versions list

`remote/tested.json` on `main` keeps a format every released 1.x install
can parse, because installs download it. That is why the file still holds
a frozen `codex` range that the current code ignores. Installs of 0.9
read it.

## What may change in any release

- The wording of messages, tool descriptions and the handoff skill's
  text. Don't parse them.
- The names and number of the steps a move reports, and how long they
  take.
- The time budgets, such as the 80 seconds a handoff has to copy.
- The commands Claude runs for you, `magi arm`, `magi wait` and
  `magi tasks wait`, and every other argument of the magi program. They
  aren't an interface for you or for scripts.
- Every other file in `~/.magi`, including the journals in `moves/`,
  `handoffs.jsonl`, `armed/`, `wanted/`, `watchers/`, `tabs/` and
  `tasks/`.
- The scripts magi sends to other machines over SSH, and the git refs
  under `refs/magi/` it keeps in a checkout.
- The default `exclude` and `secrets` lists that apply when your config
  has none. Set your own if you depend on them.
- Which Claude Code versions magi accepts. The list in `tested.json`
  moves every night.
- Which platforms are tested. [support.md](support.md) has the current
  table.

## Machines on different versions

Run the same magi version on machines that work together. When you set a
machine up or check it, `setup_machine` updates the plugin there if it is
older than the one you run it from. A 1.x release won't promise that two
different versions can move a session between them.

## What magi can't promise

magi reads Claude Code's own session files and uses its hooks. Those are
not documented interfaces, and a Claude Code release can change them. The
tested versions list exists for that. When a new Claude Code breaks a
move, magi refuses that version with a message until a fix is out. That
refusal isn't a breaking change in magi.
