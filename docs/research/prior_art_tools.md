# Prior art: tools that move, sync, share, remotely control or orchestrate coding-agent sessions across machines

Scope note: GitHub stats below were pulled live with `gh api repos/<owner>/<repo>` on 2026-11-14 (stars, SPDX license, last push, archived flag, latest release). Where a figure has no other source, the source is the repo URL itself. Architecture claims are cited to READMEs and docs I fetched. Items I could not verify this session are listed under Gaps rather than stated as fact.

## Q1. What prior art exists, and how does each tool work? (catalogue)

### Takeaway
The field splits into five groups: (a) agent-on-host plus thin remote client (T3 Code, Happy, Claude Code Remote Control, CloudCLI/claudecodeui, `opencode serve`/`attach`); (b) cloud-sandbox agents that hand work back through git branches or diffs (Claude Code cloud sessions + `--teleport`, Codex cloud + `apply`, Cursor background agents, Terragon, Omnara's managed-agent pivot); (c) local parallel-session managers built on worktrees and tmux (claude-squad, ccmanager, Crystal/Nimbalyst, Vibe Kanban, Sculptor, container-use); (d) file-level syncers for `~/.claude` transcripts (claude-sync, claude-code-sync, claude-history-sync, twin); (e) terminal persistence (tmux, zellij, mosh, Eternal Terminal, dtach, abduco, sesh). None of them moves a live session (workspace, secrets and transcript) between two arbitrary hosts.

### Cited Findings

**T3 Code (pingdotgg/t3code, Theo / Ping Labs)**
- Stats: 59,567 stars, MIT, last push 2026-11-13, latest release v0.1.9 on 2026-11-11. Tagline: "The best way to control Claude Code, Codex and more from any computer or phone" — [GitHub](https://github.com/pingdotgg/t3code)
- Runs the coding agents on your machine behind one interface. Supports Claude Code, Codex, Cursor, OpenCode and any ACP agent. "One server, and every client shows the same threads." Clients are desktop, web (app.t3.codes), iOS and Android. Each thread can work in its own worktree — [README](https://github.com/pingdotgg/t3code)
- Architecture principle: "T3 Code keeps execution in the environment that owns the workspace… A remote client must never substitute its own filesystem, provider credentials, or machine state for the environment's." The desktop app bundles a server — [docs/internals/overview.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/overview.md)
- State is event-sourced. A v2 orchestrator serialises commands into events, and an EventSink commits events, projections, command receipts and outbox effects in one database transaction. An effect worker does side effects after intent is recorded. The event log keeps only "about a day of history for reconnect replay", and older events are pruned, so "projections cannot be rebuilt from the log". Checkpoints use hidden Git refs to capture workspace state without adding commits — [overview.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/overview.md). The repo also has SQLite test fixtures — [repo tree](https://github.com/pingdotgg/t3code)
- Remote model: "Each connection joins a client to one environment over HTTP and WebSocket. The environment owns providers, execution, files, and durable state. Direct access, Tailscale, SSH, and T3 Connect change how the client reaches that server; they do not introduce another execution model." — [docs/internals/remote.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md)
- No migration between hosts: "A repository identity can correlate clones across environments, but never routes work between them. A project and its threads belong to one environment; an agent reaches another environment's threads only through a peer link." — [remote.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md)
- Peer links (beta, `linkedEnvironmentsEnabled`): one environment signs in to another environment's `/mcp` endpoint as an OAuth MCP client, so its agents can run tools or threads there — [remote.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md)
- Route handling: a saved environment keeps an ordered list of routes (LAN, tailnet, T3 Connect), and the client walks them in order. Learned LAN and tailnet routes reuse the existing credential. A headless background service accepts connections only from loopback, private or tailnet IPv4 peers — [remote.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md)
- T3 Connect relay: Clerk identity; a Cloudflare relay manages environment links and managed tunnels. "After bootstrap, clients send application traffic through the environment's tunnel hostname; the relay Worker does not proxy their HTTP or WebSocket sessions." The relay brokers a one-time bootstrap credential bound to the client's DPoP key and never receives the session token. Optional webhook holding uses a Durable Object with SQLite storage — [docs/internals/t3-connect.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/t3-connect.md)
- Cross-provider handoffs (for example Claude to Codex) are textual summaries that "do not claim native provider context parity"; each text item is compacted to at most 240 characters — [docs/internals/context-handoffs.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/context-handoffs.md)

**Happy (slopus/happy, happy.engineering)**
- Stats: 24,629 stars, MIT, last push 2026-11-13, latest tagged CLI release cli-1.2.5 on 2026-09-22. The separate `slopus/happy-server` repo (367 stars) is archived; the relay now lives in the monorepo at `packages/happy-server` — [GitHub](https://github.com/slopus/happy), [happy-server](https://github.com/slopus/happy-server)
- Now positioned as "Orchestrate Claude, Codex and Grok in one app", with multiplayer sessions, an E2E-encrypted mobile app, a desktop app (`slopus/happy-desktop`) and an agent runtime (`slopus/happy-agent`). The original `happy claude` / `happy codex` CLI wraps the harness in your terminal so you can continue from your phone — [README](https://github.com/slopus/happy)
- Architecture: the CLI has a daemon that "runs in the background, spawns sessions, and maintains machine state". It talks HTTP plus Socket.IO to the server. Local state lives in `~/.happy` (settings.json, access.key, daemon.state.json) — [docs/cli-architecture.md](https://github.com/slopus/happy/blob/main/docs/cli-architecture.md)
- Encryption: clients encrypt, and the server stores opaque blobs in Postgres ("Keep the server blind to user content"). There are two variants: legacy NaCl secretbox (XSalsa20-Poly1305) with a 32-byte shared secret, and AES-256-GCM with per-session or per-machine data keys — [docs/encryption.md](https://github.com/slopus/happy/blob/main/docs/encryption.md)
- The repo keeps a `docs/competition` folder (claude, codex, opencode, superset, comparison-matrix.md), which is useful for a landscape view — [repo tree](https://github.com/slopus/happy/tree/main/docs/competition)

**Claude Code Remote Control (Anthropic)**
- Connects claude.ai/code or the Claude mobile app to a Claude Code session running on your machine. "Claude keeps running locally the entire time." The conversation stays in sync across terminal, browser and phone, and the session reconnects after sleep or a network drop. `claude remote-control` (alias `claude rc`) is server mode; there is also `--remote-control` and `/remote-control` — [docs](https://code.claude.com/docs/en/remote-control)
- Transport: "outbound HTTPS requests only and never opens inbound ports… registers with the Anthropic API and polls for work." Traffic goes over TLS through the Anthropic API with short-lived, purpose-scoped credentials. While connected, the transcript is stored on Anthropic servers. It is not E2E encrypted the way Happy is, and it is unavailable for ZDR or HIPAA orgs, API-key auth, Bedrock/Vertex/Foundry, or a custom `ANTHROPIC_BASE_URL` — [docs](https://code.claude.com/docs/en/remote-control)
- Server mode can spawn new sessions in `same-dir` or `worktree` mode. If the session is resumed in a second terminal, Remote Control stays with the first unless you run `/remote-control` in the second — [docs](https://code.claude.com/docs/en/remote-control)

**Claude Code cloud sessions ("Claude Code on the web") + `--cloud` + `--teleport` (Anthropic)**
- Cloud sessions run in isolated Anthropic-managed VMs or in an org's self-hosted environment. Git credentials stay outside the VM behind a GitHub proxy — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- "From the CLI, session handoff is one-way: you can pull cloud sessions into your terminal with `--teleport`, but you can't push an existing terminal session to the cloud." `--cloud` starts a new cloud session. "The Desktop app can send a local session in its Code tab to the cloud from its Open in menu." — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- `--cloud` clones the GitHub remote at the current branch, not the local checkout. With no remote or no GitHub App, it uploads a git bundle (all branches plus uncommitted changes to tracked files, under 100 MB, untracked files excluded). On macOS/Linux/WSL, uncommitted changes to `.env`, `*.tfvars`, `id_rsa` and `*.pem` are left out of the bundle — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- Teleport needs a clean git state (it offers to stash), the same repo (not a fork), the branch pushed to the remote, and the same claude.ai account. It fetches the branch and loads the conversation. Messages summarised by compaction are not copied. "The terminal gets its own copy of the session: new work there stays local." — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- When an idle environment expires, the conversation is restored but running shell commands, subagents and session-scoped scheduled tasks are not — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)

**opencode (anomalyco/opencode, formerly sst/opencode)**
- Stats: 226,669 stars, MIT, last push 2026-11-13, latest release v1.19.5 on 2026-10-28. `sst/opencode` resolves to `anomalyco/opencode` — [GitHub](https://github.com/anomalyco/opencode)
- Client/server split: `opencode serve` runs a headless HTTP server with an OpenAPI spec, and the TUI is a client. `opencode attach <url>` points a local TUI at a remote backend, with `--session` and `--dir`. Auth uses `OPENCODE_SERVER_PASSWORD`. The server keeps running after a client detaches — [opencode docs (v1 server)](https://opencode.ai/v1/docs/server.md), [CLI docs](https://opencode.ai/docs/cli.md), [mintlify mirror: attach](https://www.mintlify.com/anomalyco/opencode/cli/attach)
- There is an open PR adding `--attach` to `acp`, `web` and `serve` — [PR #7340](https://github.com/anomalyco/opencode/pull/7340)

**CloudCLI / Claude Code UI (siteboon/claudecodeui)**
- Stats: 14,601 stars, AGPL-3.0, last push 2026-11-13, latest release v1.37.8 on 2026-11-07. A web and mobile UI for Claude Code, OpenCode, Cursor CLI and Codex — [GitHub](https://github.com/siteboon/claudecodeui)
- A local server on `localhost:3001` auto-discovers existing sessions (it reads `~/.claude`). Remote server setup is covered in its docs. The project now also sells "CloudCLI Cloud" environments, and the desktop app switches between local and remote workspaces. A plugin (`ccx`) hands an open session to another agent with its history — [README](https://github.com/siteboon/claudecodeui)

**Omnara (omnara-ai/omnara)**
- Stats: 3,395 stars, Apache-2.0, last push 2026-11-13, latest release omnarad-v0.2.23 on 2026-11-13 — [GitHub](https://github.com/omnara-ai/omnara)
- Pivoted from a mobile companion for Claude Code to "The open-source managed agent platform", a self-hostable alternative to Claude Managed Agents and the OpenAI Agents API. Agents are defined in `agent.yaml` with `machine_sources` (machine pools or managed sandboxes), plus an SDK and an event stream. It self-hosts with Docker Compose — [README](https://github.com/omnara-ai/omnara)

**Vibe Kanban (BloopAI/vibe-kanban)**
- Stats: 28,678 stars, Apache-2.0, last push 2026-11-12, last release 2026-04-24 — [GitHub](https://github.com/BloopAI/vibe-kanban)
- **Sunsetting.** Bloop announced its shutdown on 2026-04-10 ("the vast majority are free users and we couldn't find a business model"). Remote services stayed up for 30 days before the tool went fully local, and the project continues as community-maintained open source. Workspaces give each agent a branch, a terminal and a dev server — [announcement](https://www.vibekanban.com/blog/shutdown), [README](https://github.com/BloopAI/vibe-kanban)

**Crystal (stravu/crystal)**
- Stats: 3,128 stars, MIT, last release v0.3.5 on 2026-02-26. **Deprecated in February 2026** and replaced by Nimbalyst. It ran parallel Codex and Claude Code sessions in git worktrees — [GitHub](https://github.com/stravu/crystal)

**Sculptor (imbue-ai/sculptor)**
- Stats: 246 stars, MIT, latest release sculptor-v0.49.0 on 2026-10-21, active. A desktop app for parallel agents in "workspaces (an isolated copy of your code)". Labelled an experimental research preview, and not taking many external contributions — [GitHub](https://github.com/imbue-ai/sculptor)

**claude-squad (smtg-ai/claude-squad)**
- Stats: 8,802 stars, AGPL-3.0, latest release v1.0.20 on 2026-08-20. A TUI over tmux and git worktrees for Claude Code, Codex, OpenCode and Amp. It can attach and detach, and "checkout" commits the changes and pauses the session. Config lives in `~/.claude-squad/config.json` — [GitHub](https://github.com/smtg-ai/claude-squad)

**ccmanager (kbwo/ccmanager)**
- Stats: 1,278 stars, MIT, latest release v4.4.11 on 2026-10-27. Manages sessions across git worktrees for 8+ agent CLIs. It can restore sessions after a restart, "copy Claude Code session data between worktrees", carry gitignored files (`.env`, local certs) into new worktrees via `.worktreeinclude`, and integrate with devcontainers. It does not use tmux — [GitHub](https://github.com/kbwo/ccmanager)

**Terragon (terragon-labs)**
- **Shut down.** The `terragon-labs/terragon-oss` snapshot is Apache-2.0, has 261 stars and was last pushed 2026-02-10. Its README carries a shutdown notice dated 2026-01-16. Terragon was a remote background-agent orchestrator that ran Claude Code and Codex in the cloud — [terragon-oss](https://github.com/terragon-labs/terragon-oss), [secondary write-up](https://vibed-lab-agents.pages.dev/agent/terragon-labs-terragon-oss), [blog mention](https://jmduke.com/posts/failed-experiments/)

**container-use (dagger/container-use)**
- Stats: 4,090 stars, Apache-2.0, last push 2026-09-21, **last release v0.4.2 in August 2025, so releases look stalled**. An MCP server that gives each agent a fresh container on its own git branch. You can drop into the agent's terminal, and review is a plain `git checkout` — [GitHub](https://github.com/dagger/container-use)

**Codex CLI and Codex cloud (OpenAI)**
- Stats: 135,187 stars, Apache-2.0, latest release rust-v0.169.0 on 2026-11-13 — [GitHub](https://github.com/openai/codex)
- Cloud tasks return a diff that you apply locally with `codex apply` / `codex cloud apply TASK_ID`. The desktop app's automations run locally against a folder or worktree — [developertoolkit](https://developertoolkit.ai/en/workflows/automate/background-agents/), [danielvaughan guide](https://codex.danielvaughan.com/2026/03/27/codex-cloud-vs-local-when-to-run-in-cloud/) (third-party sources)

**Cursor background or cloud agents**
- Changes come back to the local editor after a cloud run, and the foreground-to-background handoff is described as smooth (an opinion, not a measurement) — [alexcloudstar comparison](https://alexcloudstar.com/blog/best-background-coding-agents-2026/), [learncursor](https://www.learncursor.dev/guides/local-agent-vs-cloud-agent) (third-party)

**Zed + ACP**
- Zed: 94,958 stars, latest v1.28.1 on 2026-11-11 — [GitHub](https://github.com/zed-industries/zed)
- Agent Client Protocol ("connecting any editor to any agent"): 4,641 stars, Apache-2.0, schema v1.31.0 on 2026-11-10 — [GitHub](https://github.com/agentclientprotocol/agent-client-protocol)
- The Claude ACP adapter (`zed-industries/claude-code-acp`, now `agentclientprotocol/claude-agent-acp`): 2,770 stars, v0.107.0 on 2026-11-13 — [GitHub](https://github.com/agentclientprotocol/claude-agent-acp)
- T3 Code and Happy both accept "any ACP agent" — [T3](https://github.com/pingdotgg/t3code), [Happy](https://github.com/slopus/happy)

**Remote dev-environment and sandbox hosts**
- Coder (coder/coder): 17,378 stars, AGPL-3.0, v2.38.7 on 2026-11-10, described as "Secure environments for developers and their agents". `coder/agentapi` ("HTTP API for Claude Code, Goose, Aider, Gemini, Amp, and Codex"; 1,495 stars, MIT) is **archived** — [coder](https://github.com/coder/coder), [agentapi](https://github.com/coder/agentapi)
- DevPod (loft-sh/devpod): 15,319 stars, MPL-2.0, **last push 2025-11-14 and last release v0.6.15 in March 2025, so it looks stalled** — [GitHub](https://github.com/loft-sh/devpod)
- Gitpod (gitpod-io/gitpod): 13,797 stars, AGPL-3.0. Its last GitHub release is from 2022, though the repo was pushed 2026-11-11 — [GitHub](https://github.com/gitpod-io/gitpod)
- Daytona (daytonaio/daytona): 71,318 stars, "Secure and Elastic Infrastructure for Running AI-Generated Code". **The repo is marked archived** (last push 2026-07-24, last release v0.190.0 on 2026-06-23) — [GitHub](https://github.com/daytonaio/daytona)
- E2B (e2b-dev/E2B): 14,850 stars, Apache-2.0, CLI 2.24.0 on 2026-11-13 — [GitHub](https://github.com/e2b-dev/E2B)

**`~/.claude` session syncers (community)**
- tawanorg/claude-sync: 325 stars, MIT, pushed 2026-11-13. Push and pull to R2, S3, GCS or WebDAV, encrypted with age; the passphrase derives the key; gzip compression; `--scope sessions`; conflict and diff commands; optional shell hooks for automatic push and pull. It translates project paths so sessions resume on machines with different paths — [GitHub](https://github.com/tawanorg/claude-sync), [search summary](https://github.com/tawanorg/claude-sync)
- perfectra1n/claude-code-sync: 103 stars, Rust. Syncs through a git (or Mercurial) repo with smart merges, a per-machine "Project Map" that pins a project to its path, neutral `__HOME__` / `__CLAUDE_DIR__` paths in config, a secrets denylist, Git LFS, and a TUI for conflicts — [GitHub](https://github.com/perfectra1n/claude-code-sync)
- Edenzzzz/claude-history-sync: 16 stars. Uses Google Drive and keys conversations by **git remote URL plus relative subdirectory** instead of absolute path. Newer mtime wins, and unique entries are merged by timestamp. Projects without a remote are skipped. It also handles Codex rollout files — [GitHub](https://github.com/Edenzzzz/claude-history-sync)
- crafter-station/twin: 0 stars, MIT, **created 2026-11-13**, so very new. Mac-to-Mac over SSH and rsync, preferring a USB-C or Thunderbolt link, then LAN, then Tailscale, then Bonjour. It syncs config, sessions (transcripts, memory, checkpoints, todos, plans), prompt history (line merge) and MCP servers (key merge), and never syncs credentials. `twin run` copies the project and its memory to the same path on the other Mac and runs headless Claude Code there. `twin fetch` brings the changes and the session back for `claude --resume`. It rewrites usernames in paths inside transcripts. Rules: "Do not open the same session on both Macs at once", and "Never deletes on the other side". `twin rc on` keeps Remote Control always on — [GitHub](https://github.com/crafter-station/twin)
- Smaller ones: peteknowsai/ccloud ("iCloud for Claude Code", 1 star), kossen6891/claude-history-sync (Google Drive, git-repo keyed), JamieEff/claude-history-sync (any synced folder), Sentiaus/claude-history-sync (self-hosted versioning) — [ccloud](https://github.com/peteknowsai/ccloud), [kossen6891](https://github.com/kossen6891/claude-history-sync), [JamieEff](https://github.com/JamieEff/claude-history-sync), [Sentiaus](https://github.com/Sentiaus/claude-history-sync)

**Terminal-level persistence**
- tmux: 50,304 stars, ISC, release 3.8 on 2026-09-09 — [GitHub](https://github.com/tmux/tmux)
- zellij: 36,042 stars, MIT, v0.46.0 on 2026-11-11 — [GitHub](https://github.com/zellij-org/zellij)
- mosh: 14,662 stars, GPL-3.0, **last release 1.4.0 in October 2022** — [GitHub](https://github.com/mobile-shell/mosh)
- Eternal Terminal: 3,947 stars, Apache-2.0, et-v7.1.1 on 2026-10-29 ("Re-Connectable secure remote shell") — [GitHub](https://github.com/MisterTea/EternalTerminal)
- dtach: 760 stars, GPL-2.0, last push June 2025 — [GitHub](https://github.com/crigler/dtach)
- abduco: 1,001 stars, ISC, **last push January 2023, effectively dormant** — [GitHub](https://github.com/martanne/abduco)
- sesh (joshmedeski/sesh, "Smart tmux session manager"): 2,931 stars, MIT, v2.34.0 on 2026-10-25 — [GitHub](https://github.com/joshmedeski/sesh)

### Inferences
- The large projects (T3 Code at about 60k stars, Happy at about 25k, opencode at about 227k) all chose "agent stays on one host; clients come to it". The bet is remote viewing and control, not migration.
- Churn is high in this category: Terragon (shut down January 2026), Vibe Kanban (sunset April 2026), Crystal (deprecated February 2026), Omnara (pivoted), coder/agentapi (archived), Daytona (GitHub repo archived) and DevPod (stalled). A new tool should not depend on a hosted service the authors must pay for. Vibe Kanban's own post blames "free users" with no business model.

### Gaps
- Live stats and architecture for the Conductor (Mac app), Cursor background agents (current name and how they hand off to local), GitHub Copilot coding agent, Gitpod/Ona, Modal Sandboxes and Zed remote development (SSH headless server) were not fetched this session. Treat any architecture claims about them as unverified.
- I could not find a `claudecode-remote` / `ccremote` repo by name in the searches I ran. These names may be small forks; they are not covered here.
- Why the Daytona GitHub repo is archived (moved repo, or relicensed?) was not checked.
- Codex "resume across machines" behaviour was not verified.

## Q2. Does anything already do true host-to-host migration of a Claude Code session (workspace + secrets + transcript) in seconds? If not, what is closest?

### Takeaway
No. Nothing found migrates a live Claude Code session between two arbitrary hosts together with its workspace, secrets and transcript. The closest options are: (1) **twin**, which copies the project, memory and transcript Mac to Mac over rsync, rewrites paths, and then resumes with `claude --resume`, but has no secrets, Macs only, and is one day old; (2) **Claude Code `--teleport`**, which goes cloud to local only, through a pushed branch and the transcript, and requires a clean tree; (3) the **Claude Desktop "Open in" menu**, which goes local to cloud; and (4) transcript syncers (claude-sync, claude-code-sync) that move the conversation but not the working tree, the running processes or the secrets.

### Cited Findings
- Anthropic's own CLI handoff is explicitly one-way: "you can pull cloud sessions into your terminal with `--teleport`, but you can't push an existing terminal session to the cloud." Teleport gives "its own copy of the session". Uncommitted work has to be pushed, and compacted messages are lost — [Claude Code docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- The `--cloud` bundle upload deliberately leaves secrets out (`.env`, `*.tfvars`, `id_rsa`, `*.pem`) and also leaves out untracked files. Secrets have to be configured separately as cloud-environment "network secrets" injected by a proxy — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- Remote Control does not move anything: "your computer has to stay on and the `claude` process has to keep running" — [docs](https://code.claude.com/docs/en/remote-control)
- T3 Code rules out cross-environment routing by design: "A project and its threads belong to one environment" — [remote.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md)
- Feature request #51337 ("Native support for continuing a session across machines", opened 2026-04-20, now **closed**) describes the core problem. Transcripts, memory and plans live under `~/.claude/projects/<project-hash>/`, "the hash is derived from the absolute cwd, so even syncing `~/.claude/` via iCloud doesn't reliably carry a session across machines unless both repos happen to live at identical paths." It proposed `claude session push` and `claude --resume --from-cloud` — [issue #51337](https://github.com/anthropics/claude-code/issues/51337)
- Feature request #73639 ("Transfer session to another claude code instance", opened 2026-07-02, **open**, 9 comments) asks that `claude --resume <session-id>` work on a different host, with transcript and project memory — [issue #73639](https://github.com/anthropics/claude-code/issues/73639)
- Issue #107236 (2026-11-04): "Desktop app: session tools cannot see or move sessions synced from other computers". The title suggests the Claude Desktop app now syncs some session metadata across computers, but I did not verify that — [issue #107236](https://github.com/anthropics/claude-code/issues/107236)
- The manual workaround is to copy the session `.jsonl` into the matching `~/.claude/projects/` directory (for example with scp) and then `claude --resume`. A guide said there was no built-in cross-machine sync for local sessions as of October 2026 — [codonic.dev guide](https://codonic.dev/guides/move-claude-code-session-to-another-computer), [aq.dev guide](https://aq.dev/guides/move-claude-code-between-laptop-and-cloud/)
- twin is the closest end-to-end attempt. `twin run` copies the project (without `node_modules` or build output) and memory to the same path on the other Mac and starts headless Claude Code under `caffeinate`. `twin fetch` brings the changes and the session back, and `twin claude` opens Claude Code on the mini in the same folder. Credentials are never synced: "each Mac signs in on its own" — [twin](https://github.com/crafter-station/twin)
- ccmanager copies Claude Code session data between worktrees on one host, and `.worktreeinclude` carries `.env` files into new worktrees. Secrets travel within a host, not between hosts — [ccmanager](https://github.com/kbwo/ccmanager)

### Inferences
- None of the tools stops the agent cleanly, ships the transcript, workspace diff, untracked files and selected secrets in one encrypted bundle, and then resumes on the target with a guarantee that only one host owns the session. twin's "do not open on both Macs" rule and Remote Control's "Remote Control not started here" behaviour show that single ownership (a lease or lock) is an unsolved problem left to the user.
- No source gave timings for any handoff. Teleport's latency is bounded by git fetch and checkout, twin's by rsync over the chosen link. No benchmark for "handoff in seconds" exists.

### Gaps
- No published latency or benchmark data for teleport, twin or the syncers.
- The comments on #73639 and Anthropic's response (if any) were not read in full. Why #51337 was closed (duplicate? shipped?) was not checked.
- Reddit and HN threads were not fetched directly this session. The community evidence here comes from the GitHub issues and guides above.

## Q3. What architectural patterns recur?

### Takeaway
The recurring patterns are: the agent runs on a long-lived host daemon with thin web, mobile and desktop clients over WebSocket or RPC; reachability through Tailscale, LAN, SSH or a relay/tunnel; E2E encryption in community relays (Happy) versus TLS-only vendor relays (Anthropic); event-sourced SQLite or Postgres stores (T3, Happy) alongside Claude's on-disk JSONL transcripts; one git worktree or container per session; and for portability, git branches, bundles or diffs as the unit of transfer, with secrets deliberately excluded.

### Cited Findings
- **Daemon on host + thin clients:** T3 Code ("one server, and every client shows the same threads"; HTTP/WebSocket RPC with a versioned contract) — [T3 overview](https://github.com/pingdotgg/t3code/blob/main/docs/internals/overview.md). Happy's daemon spawns sessions and syncs over Socket.IO — [Happy CLI arch](https://github.com/slopus/happy/blob/main/docs/cli-architecture.md). `opencode serve` plus `attach` — [opencode docs](https://opencode.ai/v1/docs/server.md). CloudCLI reads `~/.claude` from a local server — [claudecodeui](https://github.com/siteboon/claudecodeui).
- **Relay or tunnel options:** Anthropic Remote Control (outbound HTTPS polling through the Anthropic API; transcript stored server-side) — [docs](https://code.claude.com/docs/en/remote-control). T3 Connect (a Cloudflare relay brokers DPoP-bound bootstrap credentials, then traffic flows through a per-environment tunnel and the relay does not proxy it) — [t3-connect.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/t3-connect.md). Happy relay (zero-knowledge, AES-256-GCM per-session data keys, Postgres) — [encryption.md](https://github.com/slopus/happy/blob/main/docs/encryption.md).
- **Session store:** Claude Code keeps JSONL under `~/.claude/projects/<path-derived-dir>/<session-id>.jsonl` — [issue #73639](https://github.com/anthropics/claude-code/issues/73639). T3 uses an event log plus projections committed in one DB transaction, with about one day of retained events — [overview.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/overview.md).
- **Workspace isolation:** worktrees (T3, claude-squad, ccmanager, Crystal, Remote Control `--spawn worktree`), containers (container-use) and VMs (Claude cloud sessions) — [T3](https://github.com/pingdotgg/t3code), [claude-squad](https://github.com/smtg-ai/claude-squad), [ccmanager](https://github.com/kbwo/ccmanager), [container-use](https://github.com/dagger/container-use), [Remote Control](https://code.claude.com/docs/en/remote-control), [cloud](https://code.claude.com/docs/en/claude-code-on-the-web).
- **Checkpoints as hidden git refs** (T3) capture workspace state without user-visible commits — [overview.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/overview.md).
- **Transfer unit = git:** teleport fetches a pushed branch, `--cloud` uploads a git bundle, Codex cloud returns a diff to `apply` — [Claude docs](https://code.claude.com/docs/en/claude-code-on-the-web), [developertoolkit](https://developertoolkit.ai/en/workflows/automate/background-agents/).
- **Path-independent project identity:** used by claude-history-sync (git remote URL plus relative subdir), claude-code-sync (per-machine project map plus `__HOME__` tokens), twin (rewrites usernames inside transcripts) and T3 (repository identity follows the `upstream` remote) — [claude-history-sync](https://github.com/Edenzzzz/claude-history-sync), [claude-code-sync](https://github.com/perfectra1n/claude-code-sync), [twin](https://github.com/crafter-station/twin), [remote.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md).
- **Secrets excluded by default:** Claude `--cloud` bundle, claude-code-sync's "Secrets Guard" denylist, and twin (credentials never synced) — [docs](https://code.claude.com/docs/en/claude-code-on-the-web), [claude-code-sync](https://github.com/perfectra1n/claude-code-sync), [twin](https://github.com/crafter-station/twin).
- **Cross-agent handoff by summary:** T3's portable handoffs are 240-character compacted summaries with no native context parity — [context-handoffs.md](https://github.com/pingdotgg/t3code/blob/main/docs/internals/context-handoffs.md).
- **ACP as the agent abstraction:** T3, Happy and Zed accept any ACP agent, and Claude is exposed through `claude-agent-acp` — [ACP](https://github.com/agentclientprotocol/agent-client-protocol), [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp).

### Inferences
- Two state models compete. One is the agent's native transcript (Claude JSONL, Codex rollouts), which is the only thing that resumes with full fidelity. The other is a wrapper's own event store (T3, Happy). Wrappers cannot make a native session portable across hosts. Tools that sync the native JSONL (syncers, twin) get fidelity but no live control.
- Tailscale plus a daemon is the de facto self-hosted reachability layer (T3 routes, twin's route order, the user's own Rex/homelab setup). A relay is needed mainly for phones off the tailnet.

### Gaps
- Codex rollout file format and its path keying were not examined.
- Whether Happy stores full transcripts server-side (encrypted) or only live messages was not confirmed from `protocol.md`.

## Q4. What gaps would a new open-source "agent session handoff" tool fill?

### Takeaway
Gaps a new tool could fill: (1) bidirectional host-to-host moves (laptop to server, server to laptop, server to server), not only cloud to local; (2) one atomic bundle of transcript, memory, plans, the uncommitted and untracked workspace, and an explicit, encrypted secrets manifest; (3) a single-owner lease so the same session cannot run on two hosts at once; (4) path-independent project identity; (5) no vendor account or ZDR restriction, and no reliance on a hosted relay that might shut down; (6) multiple agents (Claude JSONL plus Codex rollouts) through native-format adapters rather than lossy summaries.

### Cited Findings
- Anthropic's CLI cannot push a terminal session anywhere, teleport requires a pushed branch and a clean tree, and compacted history is lost — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- Remote Control and cloud sessions are unavailable with API-key auth, Bedrock/Vertex/Foundry, custom base URLs, ZDR or HIPAA. Remote Control stores transcripts on Anthropic servers — [Remote Control docs](https://code.claude.com/docs/en/remote-control), [cloud docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- Users keep asking for native cross-machine resume (#51337, #73639) and for importing sessions from other agents (#94572, "import/sync sessions from Codex") — [#51337](https://github.com/anthropics/claude-code/issues/51337), [#73639](https://github.com/anthropics/claude-code/issues/73639), [#94572](https://github.com/anthropics/claude-code/issues/94572)
- Existing syncers are file-level with newer-wins or merge semantics. They do not move the working tree, and they rely on "don't open the same session on both machines" — [twin](https://github.com/crafter-station/twin), [claude-history-sync](https://github.com/Edenzzzz/claude-history-sync), [claude-sync](https://github.com/tawanorg/claude-sync)
- claude-history-sync skips projects without a git remote, and teleport refuses forks or mismatched repos. Identity by remote is brittle for local-only repos — [claude-history-sync](https://github.com/Edenzzzz/claude-history-sync), [docs](https://code.claude.com/docs/en/claude-code-on-the-web)
- Hosted orchestrators have a poor survival record: Terragon shut down in January 2026, Vibe Kanban's remote features went away in 2026, and Crystal was deprecated — [terragon-oss](https://github.com/terragon-labs/terragon-oss), [Vibe Kanban](https://www.vibekanban.com/blog/shutdown), [Crystal](https://github.com/stravu/crystal)
- An idle cloud VM that expires loses running shell commands, subagents and scheduled tasks; only the conversation is restored — [docs](https://code.claude.com/docs/en/claude-code-on-the-web)

### Inferences
- A strong design would combine pieces that already exist separately:
  - T3's "environment owns execution" plus route walking over Tailscale, LAN and SSH.
  - Happy's E2E-encrypted blobs if a relay is used.
  - twin's path rewriting and rsync-over-best-link.
  - claude-code-sync's secrets denylist and project map.
  - Teleport's git-based workspace transfer, extended with a bundle of uncommitted and untracked changes like `--cloud` makes.
  - A lease (for example a lock record with a fencing token) so only one host runs the session.
  - After the move, re-enable Remote Control or Rex on the target so phone access continues.
- The secrets gap is the least served. Every tool excludes secrets, and none offers an explicit, encrypted, opt-in manifest of which env vars and files travel with a session.
- Background processes (dev servers, watchers) are not moved by anything. A realistic tool would record and re-launch them (as a tmux, zellij or Rex layout) rather than migrate them live. This is an inference; no tool found does it.

### Gaps
- No sources were found on CRIU or process-checkpoint approaches for agent CLIs. A live-process migration path is unexplored in this research.
- Licensing and terms for re-hosting Claude Code transcripts (any ToS constraints on syncing `~/.claude`) were not investigated.
