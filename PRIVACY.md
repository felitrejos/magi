# Privacy

Last updated 19 November 2026.

magi is software you run on your own machines. I run no servers for it and
collect nothing. There is no telemetry, no analytics, no accounts and no
crash reports.

## What magi copies, and where

`magi` connects to machines you configured yourself, over SSH, with your
own SSH keys and config. Besides that it makes one web request, described
under "The tested versions list" below. The Claude Code plugin only runs
`magi`, so the same goes for it.

To move a session or send work, magi copies these to the machine you chose.

- The Claude Code or Codex session transcript, and for Claude Code the
  project's memory files.
- The project's git working tree, including uncommitted changes. For a
  folder that isn't a git repository, its files.
- The gitignored files that match the `secrets` patterns in your magi
  config, such as `.env`.

It doesn't send them anywhere else.

## What magi reads on your machines

To list machines and sessions, magi reads each configured machine's
system, load and memory, and the list of Claude Code and Codex sessions
running there with their titles and folders. The answers go to your
terminal or to the Claude session that asked.

## What magi keeps

magi keeps its settings in `~/.config/magi/config.toml` and its state in
`~/.magi` on each machine. That includes a log of handoffs with session
ids, machine names and folder paths, and the output of tasks you sent to
other machines. If you run `magi watch` as a service, it also writes a log
on the laptop. None of it leaves your machines.
[SECURITY.md](SECURITY.md) lists every file.

## The tested versions list

magi only moves a session with Claude Code and Codex versions its tests
passed with. The list of those versions is a file in magi's repository on
GitHub. magi fetches it with one HTTPS request to
`raw.githubusercontent.com` and keeps a copy in `~/.magi/tested.json`.

- It asks only when you move a session or run `magi doctor`, and only if
  its copy is more than an hour old.
- The request is a plain download. It sends nothing about you, your
  machines or your sessions. GitHub sees what any HTTP request carries,
  which is your IP address and a user agent.
- If the request fails, magi uses the copy it has. With no copy it uses
  the list built into the program.

## Installing

The install script downloads the release from GitHub
([github.com/felitrejos/magi](https://github.com/felitrejos/magi) releases).
That is GitHub's normal download and nothing extra.

## Other services

Claude Code, Codex, Anthropic, OpenAI, GitHub and your SSH or VPN provider
have their own privacy policies for what they do. magi doesn't change
those. A moved session runs with Remote Control, which is Anthropic's
service and shows the session in the Claude apps on your account.

## Questions

Open an issue on
[github.com/felitrejos/magi](https://github.com/felitrejos/magi/issues).
