# Support

magi moves Claude Code sessions between machines that run macOS or Linux.
It talks to them over your own SSH.

## What works

The first machine is the one a session leaves. The second is the one it
moves to.

| From | To | Status | Tested by |
|---|---|---|---|
| macOS | Linux | Supported | CI suite, nightly, the manual round trip before a release |
| Linux | macOS | Supported | CI suite, nightly, the manual round trip before a release, which brings the session back to the Mac |
| Linux | Linux | Supported | CI suite on Ubuntu and Alpine, nightly on Ubuntu. No real two-machine run |
| macOS | macOS | Supported | CI suite on macOS, nightly on macOS. No real two-machine run |
| WSL | any, or any to WSL | Not supported. `setup_machine` refuses it on either machine | Not tested |
| Native Windows | any, or any to Windows | Not supported | Not tested |

Release builds exist for macOS and Linux on arm64 and amd64. Each machine
needs git 2.31 or newer, `tar`, `curl`, tmux and Claude Code logged in
with a claude.ai account. The machine you start from needs `ssh`.

magi moves Claude Code sessions only. It accepts the Claude Code versions
in `remote/tested.json`, from the oldest supported version to the newest
one the nightly passed with.

## What each test is

- **The CI suite.** It runs on every push to `main`, on every pull
  request, and before a release can publish. It runs magi's Go tests on
  macOS and Ubuntu runners and in an Alpine container, which uses BusyBox
  tools. The tests move sessions between two home folders on one machine
  with a stand-in for `ssh` and for `claude`. They include tests that kill
  the hook at each step of a move and cut the connection. The Ubuntu job
  also runs shellcheck, staticcheck and govulncheck.
- **The nightly contract tests.** A GitHub Actions workflow runs every
  night on an Ubuntu runner and a macOS runner. It installs the newest
  Claude Code, checks the download's checksum, logs in with the repository
  secret `CLAUDE_CODE_OAUTH_TOKEN`, and runs three tests against the real
  program. They check that Claude Code records a live session where magi
  reads it, that it resumes a transcript magi moved, and that its hooks
  behave as magi expects. When both runners pass, the workflow raises the
  newest tested version in `remote/tested.json`. When one fails, it opens
  a GitHub issue that names the failed job, or adds to the open one.
- **The manual round trip.** No CI job moves a session between two real
  machines. Before a release, the owner asks Claude to "check magi on my homelab"
  from a Mac. That runs every check in `setup_machine` and its round trip,
  which moves a real Claude Code session to a Linux server and back and
  checks that it kept its memory and its uncommitted change.

## Measured

These are real runs on the owner's Mac and Linux server on 20 November
2026, with real Claude Code sessions over SSH. They are one person's
numbers on two machines, not a benchmark.

### Speed

| What | Result |
|---|---|
| Handoff, Mac to Linux | About 3 s, between 2.8 and 3.0 s |
| Hand back, Linux to Mac | About 3 s, between 2.8 and 3.0 s |
| The pull back inside `setup_machine`'s round trip | 3.3 s |

Each move reports its own time and the time of each step, so you can read
yours from the result. The times cover a small project. A large copy
takes as long as your link needs.

### When something breaks

| Test | What happened |
|---|---|
| The Stop hook was killed with `SIGKILL` in the middle of the copy | The watcher settled the move within a minute. The session stayed live on the Mac. The partial copy on the server was set aside |
| The SSH connection was frozen in the middle of the copy | The handoff gave up after about 100 s. That is the 80 s budget for the copy plus a 15 s attempt to clean up. The session kept running on the Mac. magi settled the move 40 s after the connection came back, and the next handoff worked |
| A 300 MB untracked file over a 2.7 MB/s link | The copy didn't fit the 80 s budget, and the session stayed where it was. This is why magi now shows the size and asks before a copy of 50 MB or more |

### Idle cost of the watcher

`magi watch` keeps a small loop running on each machine it watches. The
budget is 0.5% of one core and 30 MB.

| What | CPU | Peak memory |
|---|---|---|
| The watch loop on Linux | 1.47 s of CPU over 603 s, which is 0.24% of one core | 2.1 MB |
| The watch loop on the Mac | 0.30% to 0.45% of one core | 2.8 MB |
| The magi program on the Mac | 0.013% of one core | 13.5 MB |

Each number comes from a run of about ten minutes with no session
changing state. The Linux figure is the CPU time the loop's processes
used, divided by the time that passed. The Mac numbers are from an
earlier run than the Linux one.

## Known limits

- Per-session CPU in `~/.magi/live` includes child processes that already
  exited on Linux. macOS doesn't expose that to other processes, so on a
  Mac a child that started and exited between two samples isn't counted.
- A Mac to Mac or Linux to Linux move has never run between two real
  machines in a recorded test.
- Releases aren't signed. The launcher checks the download against the
  release's own `checksums.txt`.
