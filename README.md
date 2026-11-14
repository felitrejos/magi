# cielago

Tell your Claude Code session "hand this off to my homelab" and a few seconds later it is running there, with its whole conversation, your branch, your uncommitted changes and your `.env` files, and you pick it up from the Claude app through Remote Control. The copy on your laptop closes.

Named after the Fremen messenger bat that carries a message from one sietch to another.

## How it works

1. The `handoff` skill runs `cielago arm <host> <session-id>` and the agent ends its turn.
2. When the turn is over, the plugin's Stop hook runs `cielago hook stop`, which:
   - finds the same repository on the host by its `origin` remote, so paths can differ between machines;
   - checks Claude Code already trusts that folder there;
   - pushes your commits over SSH and checks out your branch (refusing if the host has uncommitted work or commits you don't have);
   - copies changed, new and deleted files plus files matching `secrets`, such as `.env`;
   - copies the session transcript into the host's `~/.claude/projects`;
   - starts `claude --resume <id> --remote-control <repo>` in a Rex pane or a tmux window, with a first message telling Claude where it is now;
   - waits until Remote Control is up, then closes the local session and renames its transcript to `*.handed-off-to-<host>` so it can't be resumed twice. A terminal session is stopped; a session in the Claude desktop app says where it went and archives itself, so only the new one is left in your sidebar.
3. If anything fails, the hook tells the agent what went wrong and the session keeps running where it was.

A handoff takes about 2.5–3.5 seconds on a LAN over Tailscale, most of it Claude starting up on the other side.

## Install

```bash
go install github.com/felitrejos/cielago@latest
claude plugin marketplace add felitrejos/cielago
claude plugin install cielago@cielago
```

On the target host, Claude Code must be logged in with `claude auth login` (Remote Control does not work with `setup-token` tokens), and each project folder must have been opened with `claude` once to accept the trust prompt.

## Configure

`~/.config/cielago/config.toml`:

```toml
[hosts.homelab]
ssh = "homelab"                            # ssh destination, key-based login
claude = "~/.local/bin/claude"             # claude binary on the host
roots = ["~/developer"]                    # where your clones live (up to two folders deep)
secrets = [".env", ".env.*", ".dev.vars"]  # gitignored files to bring along
launcher = "rex"                           # "rex" or "tmux"
rex_server = "homelab"                     # rex: the host's Rex server; the session opens beside the window named after the repo
# tmux_session = "cielago"                 # tmux: session to open a window in
```

## Limits

- Hands off between turns only, never mid-reasoning or mid-tool-call.
- Moves one way per run. To come back, the host needs SSH access to the laptop.
- Background shells, dev servers and MCP processes are not moved; they start fresh on the host.
- Staged-versus-unstaged state is not kept: changes arrive unstaged.
- Secret files from the laptop replace the host's copies, so host-specific values in a host `.env` are overwritten.
- Claude's per-project memory folder on each machine stays separate.
- `node_modules` and other ignored build output are not copied; reinstall on the host if lockfiles changed.

## Research

`docs/research/` holds the survey behind the design: Claude Code session internals, agent protocols (A2A, ACP, MCP, AG-UI), prior art, live-migration theory and secrets handling.
