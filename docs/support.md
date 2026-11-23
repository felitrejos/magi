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

The menu bar app needs macOS 26 or later, on Apple silicon or Intel. It
is built and tested on macOS 27, and hasn't run on macOS 26. It opens a
session in the Claude desktop app, Rex, Ghostty, Terminal or a tmux pane.
A session in another terminal app opens through tmux or its Remote
Control link.

- **The CI suite** runs magi's Go tests on every push to `main`, every
  pull request and before a release, on macOS, Ubuntu and Alpine with
  BusyBox tools. The tests move sessions between two home folders on one
  machine with a stand-in for `ssh` and `claude`. They kill the hook at
  each step of a move and cut the connection. A macOS job builds the
  menu bar app and runs its tests.
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

Idle cost on the Mac, over five minutes on 21 November 2026 with the
menu bar app's panel closed, one other machine set up and Claude
sessions running on the Mac. CPU is seconds of CPU time in those 300 seconds, and memory is
the footprint macOS reports.

| What | CPU seconds | Of one core | Memory |
|---|---|---|---|
| The menu bar app | 0.054 | 0.018% | 46.1 MB |
| The `magi watch` program | 0.081 | 0.027% | 10.0 MB |
| The commands `magi watch` ran | 0.821 | 0.27% | |
| The watch loop for the Mac, with the commands it ran | 2.298 | 0.77% | 1.8 MB |
| The SSH connection to the other machine | 0.000 | 0% | 2.3 MB |

The watcher's parts add up to 3.2 CPU seconds, 1.07% of one core. The
numbers come from `proc_pid_rusage`, read every 30 seconds, which also
counts the commands a process ran and waited for. The app woke 133
times, about once every two seconds.

Since 1.1 the watch loop runs a few more short commands on each stats
tick, for the monitor's lists. Two ten-minute runs side by side on the
Mac measured the watcher at 0.88% of one core before and 0.90% after,
which is within noise. Each stats line grew from about 165 bytes to
about 1 KB, once every 32 seconds or so. The table above is from before
that change.

The watch loop on Linux used 0.24% of one core and 2.1 MB over ten
minutes on 20 November 2026, before it read CPU, temperature and
network, and before the lists. It hasn't been measured since.

## Known limits

- No CI job moves a session between two real machines, and Linux to Linux
  and Mac to Mac have never run that way.
- The `magi` program isn't signed. The launcher's checksum comes from
  the same release as the binary.
- The menu bar app is signed with magi's own certificate and isn't
  notarized. A copy downloaded with a browser is blocked by macOS, and
  some managed Macs block it however it arrives.
- Installing the app needs an administrator account. The install hasn't
  run on a new Mac account or a standard one, and no update has come
  through a published release yet.
- The app has no Time Sensitive notifications, so a Focus holds them
  back.
- iTerm2 and other terminal apps come to the front as an app, not at
  the session's window, and only for a session in tmux.
- The nightly depends on one Claude account, and only raises the newest
  tested version. The list of failing versions is edited by hand.
- On macOS, per-session CPU misses a child process that started and
  exited between two samples. Linux counts it.
- The monitor's lists refresh about every 30 seconds, and a process's
  CPU is its average over that time. On a Mac the CPU card's figure is a
  1-second sample, so the two can differ during a burst.
- On Linux a process's name is the kernel's, cut at 15 characters.
- On Linux the volume list leaves out network and FUSE filesystems. It
  keeps `fuseblk`, such as an NTFS disk. It leaves out squashfs, which
  snaps use, and overlay. It keeps one mount per device, so a btrfs
  subvolume such as `/home` shows as `/`.
- Disk temperatures from `drivetemp` aren't read, so disks can spin
  down.
- The temperature list is Linux only. macOS gives no sensor readings
  without root, so on a Mac that card opens no list.
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
  A session's `tmux` field is a tmux pane id such as `%3`, not a window
  id.
- The lines of `~/.magi/handoffs.jsonl`, which can gain new fields.

Anything else can change in any release. That includes message wording,
time budgets, the magi program's own arguments, the menu bar app's look
and settings, the other files in `~/.magi`, the default `exclude` and `secrets`, and which Claude Code
versions magi accepts. Run the same magi version on machines that work
together. Claude Code's session files and hooks aren't documented
interfaces, so a new Claude Code can break a move. magi then refuses that
version until a fix is out.
