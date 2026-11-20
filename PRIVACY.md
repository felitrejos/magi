# Privacy

Last updated 20 November 2026.

magi is software you run on your own machines. It has no servers, no
telemetry, no analytics, no accounts and no crash reports. Nobody
collects anything from it.

## What magi copies

magi connects to machines you configured yourself, over SSH, with your own
keys. To move a session or send work it copies these to the machine you
chose, and nowhere else.

- The session transcript and the project's memory files.
- The project's working tree, with uncommitted and untracked files.
- The gitignored files that match your `secrets` patterns.

Before a move magi lists what goes, and Claude asks you first when secret
files, untracked files, or 50 MB or more would go.
[SECURITY.md](SECURITY.md) has the exact rules, the defaults and what
stays behind.

## What magi reads and keeps

To list machines and sessions, magi reads each configured machine's
system, load, memory and disk, its process list, and its Claude Code
sessions with their titles, folders and state. The answers go to the
Claude session that asked and to files in `~/.magi` on your own machine.

magi keeps its settings in `~/.config/magi/config.toml` and its state in
`~/.magi` on each machine. That includes a log of handoffs with session
ids, machine names and folder paths, and the output of tasks you sent.
None of it leaves your machines. [SECURITY.md](SECURITY.md) lists every
file.

## Requests to GitHub

magi makes two kinds of web request, both plain HTTPS downloads from
GitHub. They send nothing about you, your machines or your sessions.
GitHub sees your IP address and a user agent.

- The plugin downloads the magi program for its version from the
  project's GitHub releases, on first run and after each plugin update.
- magi downloads the list of Claude Code versions it was tested with,
  at most once an hour, when a session moves or you set up or check a
  machine.

## Other services

Claude Code, Anthropic, GitHub and your SSH or VPN provider have their own
privacy policies. A moved session runs with Remote Control, which is
Anthropic's service and shows the session in the Claude apps on your
account.

Questions go to the [issues](https://github.com/felitrejos/magi/issues).
