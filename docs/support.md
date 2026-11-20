# Support

magi moves Claude Code sessions between machines that run macOS or Linux,
over your own SSH.

## What works

| From | To | Status | Tested by |
|---|---|---|---|
| macOS | Linux | Supported | CI suite, nightly, manual round trip |
| Linux | macOS | Supported | CI suite, nightly, manual round trip |
| Linux | Linux | Supported | CI suite on Ubuntu and Alpine, nightly on Ubuntu. No real two-machine run |
| macOS | macOS | Supported | CI suite and nightly on macOS. No real two-machine run |
| WSL or native Windows | any, or the reverse | Not supported. `setup_machine` refuses WSL | Not tested |

Release builds exist for macOS and Linux on arm64 and amd64. The
[README](../README.md#install) lists what each machine needs. magi accepts
the Claude Code versions in `remote/tested.json`, from the oldest
supported to the newest the nightly passed with.

- **The CI suite** runs magi's Go tests on every push to `main`, every
  pull request and before a release, on macOS, Ubuntu and Alpine with
  BusyBox tools. The tests move sessions between two home folders on one
  machine with a stand-in for `ssh` and `claude`. They kill the hook at
  each step of a move and cut the connection.
- **The nightly** installs the newest Claude Code on an Ubuntu and a
  macOS runner and runs three contract tests against it. They check that
  Claude Code records a live session where magi reads it, resumes a
  transcript magi moved, and runs its hooks as magi expects. When both
  pass, it raises the newest tested version. When one fails, it opens an
  issue.
- **The manual round trip** is "check magi on my homelab" from a Mac
  before a release. It moves a real session to a Linux server and back.

## Measured

These are real runs on the owner's Mac and Linux server on 20 November
2026. They are one person's numbers on two machines, not a benchmark.

| What | Result |
|---|---|
| Handoff, Mac to Linux, small project | 2.8 to 3.0 s |
| Hand back, Linux to Mac | 2.8 to 3.0 s |
| Stop hook killed with `SIGKILL` during the copy | The watcher settled the move within a minute. The session stayed live on the Mac |
| SSH frozen during the copy | The handoff gave up after about 100 s and the session kept running on the Mac. magi settled the move 40 s after the connection came back |
| A 300 MB untracked file over a 2.7 MB/s link | The copy didn't fit the 80 s budget, and the session stayed where it was |

The watcher's idle cost, over about ten minutes with no session changing
state.

| What | CPU, of one core | Peak memory |
|---|---|---|
| The watch loop on Linux | 0.24% | 2.1 MB |
| The watch loop on the Mac | 0.30% to 0.45% | 2.8 MB |
| The magi program on the Mac | 0.013% | 13.5 MB |

## Known limits

- No CI job moves a session between two real machines, and Linux to Linux
  and Mac to Mac have never run that way.
- Releases aren't signed. The launcher's checksum comes from the same
  release as the binary.
- The nightly depends on one Claude account, and only raises the newest
  tested version. The list of failing versions is edited by hand.
- On macOS, per-session CPU misses a child process that started and
  exited between two samples. Linux counts it.
- If the watcher is killed outright, the files in `~/.magi/live` keep
  their last content. A reader treats a `heard` time older than about 40
  seconds as the machine not answering.

## Versions

The plugin's version is the version of everything, and a release tag
must match it. Until 1.0, any release can change anything. From 1.0 on,
breaking one of these needs 2.0.

- What you ask for. "Hand this off to" and "hand this back" move the
  session you are in, a failed move leaves it running where it was, and
  the copy it left refuses new messages. Claude still asks first as it
  does today, and a `.magi.toml` runs nothing until you approve it.
- The tools' names and existing inputs. A delegated Claude keeps getting
  only `machines`, `sessions`, `diff` and `task_status`.
- The config keys `local_roots`, `exclude`, `ssh`, `claude`, `roots`,
  `secrets`, `max_agents`, `min_free_memory` and `quiet_hours`, and
  `setup`, `services` and `ports` in `.magi.toml`. A key magi no longer
  uses is ignored.
- The fields of `~/.magi/live/<machine>.json`, which can gain new ones.
- The lines of `~/.magi/handoffs.jsonl`, which can gain new fields. On
  a push or pull line, `bytes` is what the move sent from one machine to
  the other: the size of the git pack, as git counts it while writing or
  receiving it, plus every tar stream magi pipes across for files,
  secrets, Git LFS objects, the transcript and project memory. Ref
  negotiation and SSH's own overhead aren't counted. `seconds` is the
  time the sync and transcript steps took.

Anything else can change in any release. That includes message wording,
time budgets, the magi program's own arguments, the other files in
`~/.magi`, the default `exclude` and `secrets`, and which Claude Code
versions magi accepts. Run the same magi version on machines that work
together. Claude Code's session files and hooks aren't documented
interfaces, so a new Claude Code can break a move. magi then refuses that
version until a fix is out.
