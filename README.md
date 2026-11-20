<p align="center">
  <img src="assets/magi.png" alt="magi" width="80">
</p>

<h1 align="center">magi</h1>

<p align="center">
  <b>Move a live Claude Code session to another machine and back, with the conversation and your uncommitted work.</b>
</p>

<p align="center">
  <img src="assets/handoff.gif" alt="A Claude Code session on a Mac runs a small server on localhost:3000. The user types hand this off to my homelab. When the turn ends, the session moves to the homelab with the server's uncommitted edit, and the Mac's localhost:3000 now answers from the homelab. Claude there remembers the change and says it is uncommitted. Told to hand this back to the Mac, the session returns." width="820">
</p>

magi also lets Claude send work to your other machines and get the result
back in your working tree. It is one program, and it talks to your
machines only over your own SSH.

## Install

Each machine needs git 2.31 or newer, `tar`, SSH that works without a
password prompt, and Claude Code signed in with a claude.ai account. A
machine that receives a session needs tmux. Run this on every machine.

```bash
curl -fsSL https://github.com/felitrejos/magi/releases/latest/download/install.sh | sh
claude plugin marketplace add felitrejos/magi && claude plugin install magi@magi
```

Then run this on your laptop.

```bash
magi try
```

`magi try` sets magi up if needed, checks both machines, then moves a
throwaway session to the other machine and back and removes it. It takes
about 15 seconds and three short Claude turns on your account. If a check
fails, it stops and says what to fix. `magi init` is the setup alone, and
you run it again to add a machine. `magi doctor` checks both ends.

If your laptop takes no incoming SSH, run `magi watch install` on it as
well, and again after you add a machine. It keeps `magi watch` running
from login, so sessions on your other machines can be handed back to the
laptop, and it keeps a live view of each machine for the menu bar app.

## Hand off and back

Say "hand this off to my homelab" in a Claude Code session. When the turn
ends, magi copies the conversation, the branch, staged and unstaged
changes, the gitignored files you listed and Claude's project memory.
Before it copies anything, magi shows what goes across, with secret files
by name only. The session continues in a tmux window on the homelab with Remote Control on,
so it is also in the Claude app, and magi prints the command that attaches
to it. The copy you left closes and refuses new messages.

Say "hand this back to my Mac" there to return. Without `magi watch`, run
`magi pull homelab` on the Mac instead. From outside a session,
`magi move <session-id> <host>` moves a session on this machine, and that
is how you move a Codex session. A handoff that fails leaves the session
running where it was, and `magi status` shows what happened.

Nothing that is running moves. A project's `.magi.toml` can name a `setup`
command, `services` to start beside the session and `ports` to forward
back, such as `ports = [3000]`. It runs only after you approve the file
with `magi allow`.

## Send work

<p align="center">
  <img src="assets/background-work.gif" alt="In a Claude Code session on a Mac, the user asks for a background Claude to build a linux/amd64 Docker image on a machine that runs amd64 natively, and meanwhile to add a /stats route on the Mac. Claude proposes the homelab and the user agrees. The Mac session writes /stats while a background Claude on the homelab writes a Dockerfile and builds the image. When it finishes, the Dockerfile and .dockerignore land in the Mac's working tree, and git status shows the uncommitted changes." width="820">
</p>

The plugin gives Claude tools to run a shell command, a background Claude
or a new session on another machine. Claude proposes a machine and waits
for your yes. The work runs in its own copy of your working tree, in a
tmux window you can watch. A background Claude's changes are applied to
your working tree and kept on the branch `magi/task-<id>`. If they clash
with your edits, they stay on the branch.

The tools are `machines`, `sessions`, `diff`, `run_elsewhere`, `delegate`,
`session_elsewhere`, `move_session`, `task_status`, `task_result`,
`task_reply` and `cancel_task`. `magi mcp` serves them to any MCP client.
Under a host in `~/.config/magi/config.toml` you can set `max_agents`,
`min_free_memory` and `quiet_hours`, and magi starts nothing new on a
machine past them.

## Commands

```text
magi init                          set up this machine and a machine to hand off to
magi try [host]                    set up if needed, check everything, then move a
                                   throwaway session to <host> and back
magi doctor [host...]              check that everything a handoff needs is in place
magi pull <host> [session-id]      bring a session back from <host> and continue it here
magi move <session-id> <host> [--json]
                                   move a session on this machine to <host>
magi wait <session>                wait until a Claude Code or Codex session on
                                   any machine isn't busy, then print whether
                                   it's idle, waiting on you or ended
magi status                        recent handoffs, parked checkouts, and moves that
                                   were cut off, which it settles
magi watch                         take back sessions other machines hand back here
magi watch install | uninstall     run magi watch in the background from login
magi unpark [dir]                  stop treating a checkout as parked
magi allow [dir]                   let the project's .magi.toml run after handoffs
magi forwards                      ports this machine forwards to sessions that moved
magi forwards close <session-id>   stop forwarding a moved session's ports
magi tasks                         work sent to other machines, and its state
magi tasks wait | show | cancel <id>
magi tasks clean                   collect finished tasks and drop old copies
```

## FAQ

**How do I move a Codex session?** With `magi move` or `magi pull`. The
other machine needs Codex installed, and `magi init` asks for its path. If
another Codex is open in the same folder, quit it first.

**Why does magi refuse my Claude Code version?** It only moves a session
with versions its tests passed with. New releases are tested every night,
so a refusal usually clears within a day without a magi update.

**What if a move is cut off?** Run `magi status`. It works out which copy
is the live one, closes the other without deleting any folder, and prints
the command to continue. The next move of that session does the same.

**Do my secrets move?** Only the gitignored files you listed, such as
`.env`, and only to machines you configured. Logins and SSH keys never
move. [SECURITY.md](SECURITY.md) and [PRIVACY.md](PRIVACY.md) list the rest.

What's next is in [ROADMAP.md](ROADMAP.md). Licensed [MIT](LICENSE).
