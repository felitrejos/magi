# Rex and Claude Code extension points for a "/handoff homelab" tool

Scope note: Rex facts come from the installed CLI on the Mac (`rex version` = 0.1.0-preview.18, commit baf79e0, built 2026-11-12, API revision 13), its built-in agent guide (`rex __internal skill`, which `~/.claude/skills/rex/SKILL.md` defers to), `rex api list` / `rex api describe` against the local server, and the public Rex docs. Claude Code facts come from code.claude.com docs and the installed CLI (`claude --version` = 2.1.319). Local CLI output is cited as "local CLI"; it has no URL. The only homelab command run was the allowed read-only `rex -S homelab ls --json`.

## 1. What Rex is, and how a tool can use it to (1) open the right tab or pane, (2) start `claude --resume`, (3) move files, (4) let the user watch

### Takeaway
Rex (by Superlogical, closed beta) is a client/server terminal multiplexer. Its CLI and JSON control API can reach the homelab's Rex server over Tailscale with `-S homelab`. They can find the project session and repo tab, split a labelled pane, run `claude --resume <id>` in it, wait on it, capture its output, and make the Mac app show it. Rex has no CLI command for copying files. The server API has `download_begin`/`upload_begin` byte streams meant for drag and drop, and port forwarding (`rex forward`) is in the docs but not in the installed build. Files should go over rsync/git through Tailscale.

### Cited Findings
**What Rex is**
- Rex calls itself "the multiplexer for all work". It is in beta: the macOS app and the Linux/FreeBSD server are open to all testers, and the iOS, Linux and Windows clients exist "at varying stages of readiness" but "aren't part of testing yet". — [Rex docs: Installation](https://www.superlogical.com/rex/docs/installation.md)
- Rex has no telemetry, crash reporting, or account/licence checks. — [Rex docs index (Privacy & Telemetry)](https://www.superlogical.com/rex/docs/llms.txt)
- On Linux the server listens only on Unix sockets by default, and remote access is set up separately. The server has built-in Tailscale (`rex server tailscale on|off|login|rename|pair`; API `server.tailscale.enable`). — [Rex docs: Remote Access](https://www.superlogical.com/rex/docs/linux/remote-access.md); local CLI `rex server tailscale --help`
- On the Mac, `rex hosts check` shows `homelab` and `homelab-rex`, both `reachable` at `https://homelab-rex.<tailnet>.ts.net`. — local CLI
- `rex -S homelab ls --json` lists sessions `scratch`, `stampr`, `autoscan`, `quorum` and `apps`. This matches the layout in the user's CLAUDE.md. — local CLI
- The CLI is not a public open-source project. I found no GitHub repository for the CLI or server. The only public repository is `evgeny-golubev/rex-extensions` (MIT), the third-party source of the installed `rex-view` Claude Code plugin. — local file `~/.claude/plugins/cache/rex-extensions/rex-view/0.5.0/.claude-plugin/plugin.json`

**(1) Finding and opening the right tab or pane on homelab**
- The Rex model: session ⊃ window (a "tab") ⊃ block (a "pane"). `-S HOST` must come before the command, using the host's full label. `$REX_SESSION` and `$REX_BLOCK` are ignored with `-S`. "A terminal on another host starts in the home directory. So does `-c` with a directory that doesn't exist, without an error". The guide says to "Change directory inside the command", for example `-- sh -c 'cd -- "$HOME/code/app" && exec npm test'`. — local CLI `rex __internal skill` (also at [Rex docs: Agents](https://www.superlogical.com/rex/docs/automate/agent-skill.md))
- Discovery commands: `rex window ls -s S --json`, `rex block ls -s S --json`, and `rex session inspect S --json`. The last one returns `active_window_id` and, for each window, `window_id` and `focused_block_id`. Labels and ID suffixes can be ambiguous, so use full IDs. — local CLI `rex __internal skill`
- Creating panes: `rex split -s S -b BLOCK --split right|left|above|below --ratio PCT -c DIR --label TEXT --focus=false --keep-open --wait --json [-- COMMAND...]`. `rex run` takes the same flags and opens a new window unless `--split` is given. `rex window new -s S LABEL -c DIR -- CMD` names a tab. `--json` returns `.block_ids` and `.window_id`. `--wait` "returns the terminal child's exit status". — local CLI `rex split --help`, `rex run --help`, `rex window new --help`
- Commands run through the login shell's environment but not its interactive config. `--interactive` also reads `~/.zshrc`; this matters if `claude` is only on PATH via rc files. `--shell=none` execs directly. — local CLI `rex run --help`
- Typing into an existing shell: `rex send -s S -b B 'text'` then `rex send-key -s S -b B enter`. The guide says to capture first and confirm the pane is at a prompt. — local CLI `rex __internal skill`
- Low-level API: `rex api call -s S session.new_split|session.new_window|session.float_block ...` with JSON args. `session.new_window` takes a `layout` tree of blocks (flavor, options, label) and splits. The only block flavors are `com.superlogical.terminal.shell` and an onboarding block. — local CLI `rex api list`, `rex api describe session.new_window`, `rex block types`
- `rex do script.lua` runs Lua against a server. `rex.server(LABEL)` "returns the same API helpers as rex, with calls going to that host", so one script can drive homelab from the Mac. — local CLI `rex do --help`

**(2) Starting and monitoring `claude --resume <id>` in that pane**
- A wrapper can start it with `rex -S homelab split -s stampr -b <shell_block> --label 'claude: handoff' --focus=false --json -- sh -c 'cd ~/developer/stampr/stampr && exec claude --resume <id>'`, then poll with `rex -S homelab capture -s stampr -b <new_block> --trim`. This is composed from the documented flags above. — local CLI help
- Rex can check the process with `rex block call -s S -b B process`, which returns the processes and `last_exit`, and `modes`, where `screen: alternate` means a full-screen TUI such as Claude Code. `rex wait -s S BLOCK --for 30s --json` waits for exit. — local CLI `rex __internal skill`, `rex wait --help`
- Terminal block methods include `pwd`, `title`, `program_status` (OSC 7501 states reported by programs), `size`, `resize` and `write`. — local CLI `rex api list`
- Rex defines a "Program Status Protocol" (OSC 7501: `state=idle|working|done|blocked|error`, `kind=permission|question|auth`). The Mac app shows working dots and needs-attention badges on tabs and can send macOS notifications. The protocol's `app` field example lists `claude-code`. — [Rex docs: Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status.md)
- Events: `rex events -s S --json` streams session events, and `rex do` scripts can `rex.on(...)` for events such as program exit. — local CLI `rex events --help`, `rex do --help`
- Rex installs a Claude Code `SessionStart` hook into `~/.claude/settings.json`: `rex __internal session-start --agent claude`. It tells the agent which Rex session and block it is in. `rex skill status` reports both the skill and the hook as `current`. — local CLI `rex skill status`; local `~/.claude/settings.json` `.hooks`; [Rex docs: Agents](https://www.superlogical.com/rex/docs/automate/agent-skill.md)

**(3) File transfer**
- The CLI has no copy, scp or exec-with-stdin command. Help lists only new/ls/run/split/do/send/send-key/capture/kill, layout commands, session/window/block/client, events/wait/api/actions, and server/config/hosts. — local CLI `rex --help`
- The server API has terminal-block methods `com.superlogical.terminal.download_begin`, which opens "a file on the terminal's machine for download" and returns a single-use ticket for the `rex.data.bytes.v1` data stream, with `if_none_match`/`version` caching, and `upload_begin`, which reserves a file upload. For uploads, "Once every file of the batch has arrived, the terminal types their paths as if the user had pasted them". So upload is designed for drag and drop into a terminal, not general sync. Both need a data-connection client (`data.connect` ticket + HELLO), which the CLI doesn't expose. — local CLI `rex api describe com.superlogical.terminal.download_begin|upload_begin|data.connect`
- `list_dir` lists a directory on the terminal's machine. — local CLI `rex api list`
- The public CLI reference documents `rex forward new (-L|-R)`, `rex forward open`, `rex forward share`, `ls` and `close`, which tunnel TCP "through a Rex server, like ssh `-L` and ssh `-R`". The installed preview.18 CLI does not have `rex forward`: it prints root help. — [Rex docs: CLI Commands](https://www.superlogical.com/rex/docs/reference/cli.md); local CLI `rex forward --help`
- The Rex docs show rsync in their own example (`rsync -a ~/Photos backup:/photos && status done ...`), meaning they expect rsync for file movement. — [Rex docs: Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status.md)

**(4) Watching from the Mac and the phone**
- Sessions live on the host's server. "They keep running on the host when this machine disconnects, and the user can open them from any client connected to that host." — local CLI `rex __internal skill`
- To show the session in the Mac app: `rex -S homelab client ls`, then `rex -S homelab -C CLIENT do session.select session_id=... [window_id=...]`. Client actions "need Remote Control, which is off by default" in the app. Don't change that setting without the user. — local CLI `rex __internal skill`
- Rex for iOS has documented actions and settings pages, but iOS is not yet in public testing. — [Rex docs: iOS Actions](https://www.superlogical.com/rex/docs/reference/ios-actions.md); [Installation](https://www.superlogical.com/rex/docs/installation.md)
- For the phone, Claude Code Remote Control is the documented path: the homelab `claude` process registers with Anthropic and shows up at claude.ai/code and in the Claude iOS and Android apps. — [Claude Code: Remote Control](https://code.claude.com/docs/en/remote-control)

### Inferences
- Rex fits the "open pane, start `claude --resume`, watch" part well, with stable, documented CLI verbs and JSON output. It is the wrong tool for moving the working tree and transcript: no CLI copy, and the API streams are drag-and-drop machinery that would mean writing a data-protocol client against a preview API.
- A self-contained alternative that keeps the user's "homelab work goes through Rex" rule: run a pull (`rsync`/`git pull`) inside a Rex pane on homelab. That needs the homelab to reach the Mac (Tailscale SSH to the Mac, or a git remote). A Mac-initiated `rsync ... homelab:` over Tailscale SSH is simpler but is "plain SSH from the Mac", which the user's CLAUDE.md reserves for when Rex is down. The tool design should ask the user which rule applies to file pushes.
- `rex forward` (once it ships to preview) could tunnel an rsync daemon or SSH port through Rex, but it is not available in the installed build.

### Gaps
- No public protocol spec for the `rex.data.bytes.v1` stream or data-connection HELLO format beyond the method schemas. I couldn't confirm a third party can implement it.
- Not verified whether Claude Code emits OSC 7501 itself, so Rex tab badges for Claude may need the user's hooks to emit it.
- Not checked: the homelab Rex server version, so I can't tell whether `rex forward` is available there.
- Rex iOS release timing is unknown.

## 2. Which Claude Code extension points let `/handoff homelab` quiesce the session, export its ID and transcript path, trigger a transfer, and stop locally

### Takeaway
A skill or command file (`~/.claude/skills/handoff/SKILL.md` with `disable-model-invocation: true`) is the right entry point. It gets `${CLAUDE_SESSION_ID}`, `$ARGUMENTS` and `${CLAUDE_PROJECT_DIR}` substituted. Its `` !`cmd` `` lines run shell commands before the model sees the prompt. Hooks provide `session_id`, `transcript_path` and `cwd` on stdin. A `Stop` hook can fire the transfer after the turn ends, and `continue:false` halts processing. No documented hook or command lets Claude Code exit its own interactive process, so "stop locally" still needs `/exit`, Ctrl-C, or a Rex `send-key` to the Mac pane.

### Cited Findings
**Skills and slash commands**
- "Custom commands have been merged into skills. `.claude/commands/deploy.md` and `.claude/skills/deploy/SKILL.md` both create `/deploy`." — [Claude Code: Skills](https://code.claude.com/docs/en/skills)
- Frontmatter fields: `disable-model-invocation`, `allowed-tools` (pre-approved for the invoking turn), `argument-hint`, `arguments`, `context: fork`, `model`, `effort`, and `hooks` ("registers when the skill is invoked and keeps running for the rest of the session"). — [Skills](https://code.claude.com/docs/en/skills)
- Substitutions: `$ARGUMENTS`, `$0`/`$1`, `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}`, `${CLAUDE_SKILL_DIR}`, `${CLAUDE_PROJECT_DIR}`, and `${CLAUDE_PLUGIN_ROOT}`/`${CLAUDE_PLUGIN_DATA}` for plugin skills. — [Skills](https://code.claude.com/docs/en/skills)
- "The `` !`<command>` `` syntax runs shell commands before the skill content is sent to Claude". Fenced ```` ```! ```` blocks allow multi-line commands, and "If a command fails, the whole skill invocation aborts". `disableSkillShellExecution` can turn this off by policy. — [Skills](https://code.claude.com/docs/en/skills)

**Hooks**
- There are 33 events, including `SessionStart`, `SessionEnd`, `Stop`, `StopFailure`, `UserPromptSubmit`, `UserPromptExpansion`, `PreCompact`, `PostCompact`, `CwdChanged`, `FileChanged`, `WorktreeCreate`, `Notification` and `PreToolUse`/`PostToolUse`. — [Claude Code: Hooks](https://code.claude.com/docs/en/hooks)
- Common input fields: `session_id`, `prompt_id`, `transcript_path`, `cwd`, `scratchpad_dir`, `permission_mode`, `effort`, `hook_event_name`. `SessionStart` `source` ∈ `startup|resume|clear|compact|fork`. `SessionEnd` reason ∈ `clear|resume|logout|prompt_input_exit|other`. `Stop` receives `last_assistant_message`. — [Hooks](https://code.claude.com/docs/en/hooks)
- `{"continue": false, "stopReason": "..."}` means "Claude stops processing entirely after the hook runs". A `Stop` hook exiting with code 2 prevents stopping. `SessionEnd` is observational with a 1.5 s default budget, up to 60 s with a longer `timeout`. — [Hooks](https://code.claude.com/docs/en/hooks)
- Hook types: `command` (with `"async": true` and `"asyncRewake": true`), `http` (POSTs the JSON input to a URL), `prompt`, `agent` (experimental) and `mcp_tool`. — [Hooks](https://code.claude.com/docs/en/hooks)
- The fetched hooks excerpt documents no hook that terminates the interactive session. — [Hooks](https://code.claude.com/docs/en/hooks)

**Sessions, transcripts and resume**
- Transcripts are stored at `~/.claude/projects/<project>/<session-id>.jsonl`, where `<project>` is the cwd with non-alphanumerics replaced by `-` (truncated and hashed beyond 200 chars). The entry format "is internal to Claude Code and changes between versions". — [Claude Code: Manage sessions](https://code.claude.com/docs/en/sessions)
- `claude --resume <session-id>` works "from any directory". The lookup checks the current project and worktrees, then "Every other project on this machine", but only "when exactly one other project holds a transcript", so "a hand-copied duplicate makes Claude Code report not-found". `claude --resume <transcript-path>` resumes from an absolute `.jsonl` path. — [Manage sessions](https://code.claude.com/docs/en/sessions)
- Resume restores conversation, model, agent, permission mode (except `bypassPermissions`), active goal and unexpired session-scoped scheduled tasks. Background Bash/monitor tasks are not restored. Flags such as `--mcp-config`, `--settings`, `--plugin-dir` and `--add-dir` must be passed again. — [Manage sessions](https://code.claude.com/docs/en/sessions)
- "If you resume the same session in two terminals without forking, messages from both interleave into one transcript." `--fork-session` gives the resume a new ID. — [Manage sessions](https://code.claude.com/docs/en/sessions)
- `CLAUDE_CONFIG_DIR` and `CLAUDE_CODE_PROJECT_DIR_NAME` (v2.1.234+) let a host pin the project directory name regardless of cwd. — [Manage sessions](https://code.claude.com/docs/en/sessions)
- On this Mac each session also has a sidecar directory `projects/<enc>/<id>/` (`subagents/`, `tool-results/`, `custom-title.json`), checkpoints under `~/.claude/file-history/<id>/`, and per-project auto memory under `projects/<enc>/memory/`. — local filesystem listing (read-only)
- The Agent SDK has `resume`, `forkSession`/`fork_session`, `continue`/`continue_conversation`, `persistSession: false` (TS), `listSessions()`/`list_sessions()`, `getSessionMessages()`, `getSessionInfo()`, `renameSession()` and `tagSession()`. Its guidance on resuming across hosts: "Session files are local to the machine that created them". Options are a `sessionStore`/`session_store` adapter ("The store lookup key derives from the working directory, so resume from a `cwd` matching the original run's"), or "Move the session file … restore it inside any directory under `~/.claude/projects/` on the new host before calling `resume`". — [Agent SDK: Work with sessions](https://code.claude.com/docs/en/agent-sdk/sessions)
- Headless: `claude -p --resume <id> --output-format json "..."` returns structured output and the session ID. — [Manage sessions](https://code.claude.com/docs/en/sessions)
- The installed CLI also has `--session-id <uuid>`, `-n/--name`, `--fork-session`, `--bg`, `attach`, `logs`, `stop`, `--teleport [session]`, `--cloud [description|session_id|url]` and `--remote-control [name]`. — local CLI `claude --help` (v2.1.319)

**Remote Control (`claude rc`)**
- `claude remote-control` (alias `claude rc`) is server mode. Flags: `--name`, `--spawn same-dir|worktree|session`, `--capacity N` (default 32), `--[no-]create-session-in-dir`, `-c/--continue`, `--session-id <id>` (v2.1.200+) and `--permission-mode`. `claude --remote-control [name]` / `--rc` is an interactive session with RC on. `/remote-control` turns it on mid-session. — [Remote Control](https://code.claude.com/docs/en/remote-control); local CLI `claude remote-control --help`
- `claude remote-control --session-id <id>` "brings back only the session whose ID you pass", where "The ID is the part of the session's URL at claude.ai/code between `/code/` and any `?`". That is the claude.ai RC session ID, not necessarily the local transcript UUID. It works "for about four hours after the server stopped". — [Remote Control](https://code.claude.com/docs/en/remote-control)
- "To bring back a session you started with `claude --remote-control` or `/remote-control`, resume the conversation with `claude --continue` or `claude --resume`." Resuming in a second terminal while the first still has RC on prints `Remote Control not started here`. Running `/remote-control` there moves it. — [Remote Control](https://code.claude.com/docs/en/remote-control)
- The **Enable Remote Control for all sessions** setting in `/config` makes every interactive process register one remote session. — [Remote Control](https://code.claude.com/docs/en/remote-control)
- Requirements: Pro, Max, Ultra, Team or Enterprise plan with a claude.ai login (no API keys). The directory must be trusted: non-TTY `claude remote-control` exits with `Workspace not trusted`. There is a one-time `Enable Remote Control? (y/n)` prompt. The local process must keep running ("start it inside `tmux` or `screen`"). Server mode exits after about 10 minutes offline. — [Remote Control](https://code.claude.com/docs/en/remote-control)
- Some commands, such as `/plugin` and `/resume`, are local-only and can't be run from the phone. — [Remote Control](https://code.claude.com/docs/en/remote-control)

### Inferences
- Proposed `/handoff homelab` design:
  - A user-only skill (`disable-model-invocation: true`) whose `` !`...` `` preamble runs a script with `${CLAUDE_SESSION_ID}` and `${CLAUDE_PROJECT_DIR}`. Because the commands run before the model is called, the transfer or staging can happen during skill expansion.
  - The transcript isn't final until the turn ends, so the safer split is: the skill marks a handoff flag file, and a skill-scoped or global `Stop` hook (with `transcript_path` and `session_id` on stdin) does the rsync and the Rex launch after the last write, then returns `continue:false` with a `stopReason` such as "Handed off to homelab".
  - An `http` or `async` hook could instead call a small daemon on the Mac.
- Stopping locally: no documented self-exit, so the options are (a) tell the user to `/exit`, (b) `rex send-key -s <mac session> -b <mac block> ctrl+c` from the hook, using `REX_SESSION`/`REX_BLOCK` from the Rex SessionStart context, or (c) run the session under `claude --bg` and use `claude stop <id>`. (b) is inside the user's own Rex setup and reliable. The Rex agent guide says "Don't send input to your own pane", so the hook needs a deliberate exception.
- Transcript path mapping: Mac cwd `/Users/felipetrejos/Developer/Personal/<repo>` encodes to `-Users-felipetrejos-Developer-Personal-<repo>`, while homelab `~/developer/<group>/<repo>` encodes differently. The tool can copy the jsonl into the homelab's correctly encoded directory, or anywhere under `~/.claude/projects/`, since cross-project ID lookup works when exactly one copy exists. It can also call `claude --resume /abs/path/<id>.jsonl`.
- Copy the sidecar `<id>/` directory too, so subagent and tool-result references resolve. `file-history/<id>/` is only needed if the user wants `/rewind` checkpoints on homelab.
- Mac absolute paths inside the conversation (`/Users/felipetrejos/...`) will be stale on homelab. Pass a first prompt on resume such as `claude --resume <id> "You are now on homelab; repo is at ~/developer/..."`.
- Remote Control after handoff: the homelab repo tabs already run `claude rc --name <repo>` (server mode). A resumed transcript doesn't automatically join that server. Running `claude --resume <id> --remote-control` (or turning on RC for all sessions on homelab) should register it with claude.ai so the phone sees it. Whether it reattaches to the same claude.ai entry or creates a new one is not documented across hosts.

### Gaps
- Not documented: whether `claude --remote-control --resume <id>` on a different machine reattaches to the original claude.ai RC session or always makes a new one. Also unconfirmed whether a `claude rc` server can adopt an existing local transcript; its `--session-id` takes the claude.ai URL ID.
- Not verified from docs: whether `--resume` and `--remote-control` combine on one command line. Both flags exist in `claude --help`, but I didn't test them, per read-only constraints.
- The `stop_hook_active` field and the rest of the `Stop` input schema were past the fetched excerpt of the hooks page.
- The details of `claude --teleport` (which session types it resumes) weren't fetched. It appears tied to cloud sessions.

## 3. Does the Claude desktop app (Code tab) support SSH remote hosts natively, and how does that compare?

### Takeaway
Yes. As of the current docs, the Code tab has SSH environments that auto-install Claude Code and a background helper on a Linux/macOS remote, and tasks keep running if the Mac sleeps or Desktop quits. But "You can't move a session that runs over SSH or in WSL" via **Open in**. The only built-in handoff is local → Cloud, and it carries a summary, not the full transcript. Moving a running local session to SSH or homelab is not offered natively.

### Cited Findings
- Setup: environment selector → **New environment** → **SSH** tab → **Add SSH connection…**, with fields Name, SSH host (`user@hostname` or a `~/.ssh/config` alias), SSH port, optional key, and Worktree folder. Then choose a remote folder and **Create environment**. — [Claude Code: Desktop](https://code.claude.com/docs/en/desktop)
- "Desktop installs Claude Code on the remote machine automatically the first time you connect", plus a helper under `~/.claude/remote/`. Requirements: x86_64/arm64, glibc 2.17+ or musl, SFTP subsystem enabled, not `noexec`, about 1 GB per user. It uses `~/.ssh/config` via OpenSSH (supports `ProxyJump`/`ProxyCommand`). — [Desktop](https://code.claude.com/docs/en/desktop)
- "If your computer sleeps, you lose your network, or you quit Desktop, the task goes on until it finishes, needs you, or reaches one of the limits", because "Claude Code runs as a process on the remote machine, under a background helper". Caveats: sign-in renewal needs Desktop running, so "don't count on a task to run overnight". The helper stops after 30 days without a connection. `KillUserProcesses`/`StopIdleSessionSec` can kill tasks. — [Desktop](https://code.claude.com/docs/en/desktop)
- SSH limitations: no Browser pane tools, **Merge when ready** disabled, and PR creation requires `git` and `gh` on the remote. Permission modes, connectors, plugins, MCP and the terminal are supported. — [Desktop](https://code.claude.com/docs/en/desktop)
- **Open in → Cloud** continues a session in the cloud "with your conversation carried over as a summary". "You can't move a session that runs over SSH or in WSL this way." — [Desktop](https://code.claude.com/docs/en/desktop)
- A deep link can create a new SSH session: `claude://code/new?ssh_host=...&ssh_port=...&ssh_folder=...&q=...`. Desktop asks for confirmation and "never sends [the prompt] for you" (v2.110.0+). — [Desktop](https://code.claude.com/docs/en/desktop)
- Desktop and CLI keep separate session lists. "The desktop app can also resume a CLI session". — [Manage sessions](https://code.claude.com/docs/en/sessions)

### Inferences
- Desktop SSH is the official way to run Claude on homelab from the Mac. But it runs under Desktop's own helper (`~/.claude/remote/`), not in the user's Rex panes, so the user can't watch it in Rex. It can only start new SSH sessions (optionally pre-filled via the `claude://code/new?ssh_host=homelab&ssh_folder=...` deep link), not move the conversation.
- The Rex-based tool gives the full-transcript handoff, visibility in Rex, and phone access through Remote Control, at the cost of maintaining transcript-copy logic against a file format the docs call internal.
- The Docs page didn't mention Tailscale. A `~/.ssh/config` alias for the homelab's Tailscale name should work through OpenSSH, but this is unverified.

### Gaps
- I didn't read the desktop page's "Coming from the CLI" section or whether a CLI session can be opened into an SSH environment. The fetched chunks didn't cover it.
- No Tailscale-specific guidance found.

## 4. What is the minimal command sequence to hand off a session today, and what is missing?

### Takeaway
Handoff is possible today with documented pieces: a skill or Stop hook gets the ID and path, rsync/git move code and transcript over Tailscale, `rex -S homelab split ... -- sh -c 'cd <repo> && exec claude --resume <id> --remote-control'` starts it, and `rex ... do session.select` shows it. Missing pieces: a supported transcript-export format, a self-exit for the local session, documented cross-host Remote Control reattachment, path remapping inside the conversation, and a Rex file-copy command.

### Cited Findings
(Each step relies on findings cited in sections 1–3. Only commands and flags documented there are used.)
- Transcript location and cross-directory resume by ID or absolute path. — [Manage sessions](https://code.claude.com/docs/en/sessions)
- "Move the session file … restore it inside any directory under `~/.claude/projects/` on the new host before calling `resume`". — [Agent SDK sessions](https://code.claude.com/docs/en/agent-sdk/sessions)
- Rex remote split, capture and client-select verbs. — local CLI `rex __internal skill`
- Interleaving if two processes write the same session. — [Manage sessions](https://code.claude.com/docs/en/sessions)

### Inferences
Minimal sequence (Mac side, run by the `/handoff homelab` skill or Stop hook). IDs and paths in angle brackets.
1. **Quiesce:** let the current turn finish (Stop hook), so the jsonl is final.
2. **Sync code:** either `git -C <repo> push` to a WIP branch, then pull in a homelab Rex pane, or `rsync -az --delete --exclude .git/index.lock --exclude node_modules <mac_repo>/ homelab:developer/<group>/<repo>/` over Tailscale SSH. The second is plain SSH, which the user's rule restricts.
3. **Copy transcript:** `rsync -a ~/.claude/projects/<mac_enc>/<id>.jsonl ~/.claude/projects/<mac_enc>/<id> homelab:.claude/projects/<homelab_enc>/` (optionally `~/.claude/file-history/<id>`).
4. **Find the target pane:** `rex -S homelab window ls -s <group> --json` → tab `<repo>`, then `rex -S homelab block ls -s <group> -w <tab> --json` → the shell block.
5. **Launch:** `rex -S homelab split -s <group> -b <shell_block> --label 'claude: handoff <id>' --focus=false --json -- sh -c 'cd ~/developer/<group>/<repo> && exec claude --resume <id> --remote-control "<repo>" "Continue; you are now on homelab at ~/developer/<group>/<repo>"'`. Use `--interactive` if `claude` is only on PATH via `~/.zshrc`. Whether `--resume`, `--remote-control` and a positional prompt combine is untested.
6. **Verify:** `rex -S homelab capture -s <group> -b <new_block> --trim` and `rex -S homelab block call -s <group> -b <new_block> process`.
7. **Show it:** `rex -S homelab client ls`, then `rex -S homelab -C <mac_client> do session.select session_id=<group_session> window_id=<tab>`. This needs Remote Control turned on in the Rex app. The phone uses the claude.ai/Claude app RC entry.
8. **Stop locally:** the user runs `/exit`, or the hook runs `rex send-key -s $REX_SESSION -b $REX_BLOCK ctrl+c` (twice for Claude's TUI, untested). Don't resume the Mac copy afterward, or use `--fork-session` if both must survive.

Missing pieces:
- (a) No supported transcript export/import. The jsonl format is internal, and the SDK's `SessionStore` adapter exists only for SDK apps, not the interactive CLI.
- (b) No documented self-exit for the interactive session.
- (c) Unknown whether RC reattaches to the same claude.ai entry across hosts. Expect a new entry.
- (d) Absolute Mac paths in history.
- (e) Uncommitted state outside the repo, such as `.env` files, local DBs and `node_modules`, needs a policy.
- (f) No Rex file-copy CLI, and `rex forward` isn't in the installed build.
- (g) The homelab `claude rc --name <repo>` servers are separate sessions. The handoff pane is in addition to them.
- (h) Workspace trust and the one-time RC confirmation must already be accepted on homelab for that directory. Non-TTY RC server mode fails with `Workspace not trusted`; interactive is fine because Rex gives it a TTY.

### Gaps
- None of the sequence was executed: it was a read-only assignment. Steps 5 and 8 in particular need testing.
- It's unknown whether the homelab has Tailscale SSH enabled, or whether the Mac accepts SSH from homelab for the pull variant. I didn't check either, per constraints.

## 5. Which surfaces are stable and documented, and which are internal or preview?

### Takeaway
Stable and documented: Claude Code skills, hooks (and their JSON fields), `--resume`/`--session-id`/`--fork-session`/`-p`, Remote Control flags, and Agent SDK session functions. Internal: the transcript jsonl format and the `~/.claude` layout beyond the documented path. All of Rex is preview/beta software: the CLI verbs and `api describe` schemas are self-documented, but the data-stream protocol, `__internal` commands, and anything Snapshot-only are not stable.

### Cited Findings
- Transcript "entry format is internal to Claude Code and changes between versions, so scripts that parse these files directly can break on any release. To build on session data, use `/export` or the script interfaces". — [Manage sessions](https://code.claude.com/docs/en/sessions)
- Agent hooks are marked experimental. — [Hooks](https://code.claude.com/docs/en/hooks)
- The TypeScript SDK's experimental V2 session API (`createSession()`) "was removed in TypeScript Agent SDK 0.3.142". — [Agent SDK sessions](https://code.claude.com/docs/en/agent-sdk/sessions)
- Rex is versioned `0.1.0-preview.18`. Its own guide says "Some commands listed in help aren't implemented yet… `rex resize` and `rex block new` aren't implemented". The docs tag Snapshot-channel features as "not yet in preview". — local CLI `rex version`, `rex __internal skill`; [Rex docs: CLI Commands](https://www.superlogical.com/rex/docs/reference/cli.md)
- The Rex skill stub on the Mac points at `rex __internal skill`, an internal subcommand whose text changes with each Rex upgrade. The SessionStart hook calls `rex __internal session-start`. — local `~/.claude/skills/rex/SKILL.md`, `~/.claude/settings.json`
- Rex states "Use only the options and behavior documented by Rex. Options from tmux don't apply." — local CLI `rex __internal skill`

### Inferences
- Build the handoff on: the skill (`${CLAUDE_SESSION_ID}`, `!` preamble), the Stop hook (`transcript_path`), `claude --resume <id|path>`, `claude --remote-control`, and Rex's documented CLI verbs (`split`/`run`/`capture`/`wait`/`block call process`/`client ... do session.select`) with `--json`.
- Treat the jsonl copy as an opaque file move: never parse or rewrite it. Pin the Claude Code version on both machines to avoid format skew, since the Mac has 2.1.319 and the homelab version is unknown.

### Gaps
- No stability or versioning policy found for the Rex API beyond "API revision 13 (supports 0 to 13)".
- The Claude Code version on homelab was not checked.
