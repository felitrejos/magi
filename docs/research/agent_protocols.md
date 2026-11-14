# Agent interoperability and agent-UI protocols as a wire format for coding-agent session handoff (as of Nov 2026)

## What is each protocol (maintainer, version/status, primitives, transport, auth, session/context/artifact model) and does it fit "session handoff"?

### Takeaway
No protocol standardises "move a running agent's full internal state from host 1 to host 2." A2A deliberately keeps agents opaque and only delegates tasks. MCP made its core stateless in July 2026. AG-UI and Agent Protocol stream state to clients. Zed's Agent Client Protocol (ACP) comes closest for coding agents: `session/load` replays a whole conversation to a newly attached client, and `session/resume` reattaches without replay. It still assumes the agent can find its own stored session, and its stable transport is local stdio only. Letta's `.af` is the only real portable agent-state file format, and it is Letta-specific.

### Cited Findings

**A2A (Agent2Agent), now under the Linux Foundation**
- Google donated A2A to the Linux Foundation in June 2025. v1.0 is the first stable spec, released under LF governance — [Turing Post](https://www.turingpost.com/p/a2a); [Linux Foundation press release](https://www.linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year)
- The release date is reported inconsistently. Turing Post says v1.0 shipped 12 March 2026, with v1.0.1 in May. An LF post on X dated 19 March 2026 announces "A2A Protocol 1.0 is here". The LF press release of 9 April 2026 calls it the first stable spec and cites more than 150 organisations. — [Turing Post](https://www.turingpost.com/p/a2a); [LF on X](https://x.com/linuxfoundation/status/2034664056125030641); [LF press](https://www.linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year)
- The official spec lists the latest version as 1.0.0, with earlier versions 0.3.0, 0.2.6 and 0.1.0. Clients negotiate the version as Major.Minor through an `A2A-Version` header — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **Core objects:** AgentCard is a JSON document covering identity, capabilities, skills, endpoint and auth. Task has `id`, `contextId`, `status`, `artifacts`, `history` and `metadata`. Message has `messageId`, `contextId`, `taskId`, `role`, `parts`, `referenceTaskIds` and `extensions`. A Part is text, a file or structured data, and an Artifact is an output made of Parts. — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **Task states:** SUBMITTED and WORKING; the interrupted states INPUT_REQUIRED and AUTH_REQUIRED; and the terminal states COMPLETED, FAILED, CANCELED and REJECTED — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **Operations:** SendMessage, SendStreamingMessage, GetTask, ListTasks, CancelTask, SubscribeToTask, create/get/list/delete for push-notification config, and GetExtendedAgentCard — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **Bindings:** JSON-RPC 2.0 over HTTPS, gRPC and HTTP+JSON/REST, plus custom bindings — [A2A spec](https://a2a-protocol.org/latest/specification/); [Turing Post](https://www.turingpost.com/p/a2a)
- **Auth:** API key, HTTP auth, OAuth2 (including device code), OpenID Connect and mutual TLS. In-task authorisation uses the AUTH_REQUIRED state, and the spec defines Agent Card signing (§8.4) — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **contextId:** it "logically groups multiple related" tasks and messages. Agents "MAY use the contextId to maintain internal state, conversational history, or LLM context", and clients treat server-generated IDs as opaque — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **Opacity is a stated design principle:** agents collaborate "without needing access to each other's internal state, memory, or tools" and need not share "internal thoughts, plans, or tool implementations". The agent decides which Messages are persisted in Task history. — [A2A spec](https://a2a-protocol.org/latest/specification/)
- `historyLength` on GetTask, SendMessageConfiguration and ListTasks only caps how many recent messages come back, and "The server MUST NOT return more messages than the provided value". Messages "MUST NOT be considered a reliable delivery mechanism for critical information" — [A2A spec](https://a2a-protocol.org/latest/specification/)
- **Extensions:** declared on the AgentCard, requested through an `A2A-Extensions` header, and failed with `ExtensionSupportRequiredError` when required — [A2A spec](https://a2a-protocol.org/latest/specification/)
- The part of the spec I read (about the first 40%) defines no mechanism for transferring conversation history or state between agents. Follow-up context is passed by reference through `referenceTaskIds` — [A2A spec](https://a2a-protocol.org/latest/specification/)

**ACP: Agent Client Protocol (Zed and the agentclientprotocol org)**
- It uses JSON-RPC between editor and agent, and `initialize` negotiates `"protocolVersion": 1` — [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- `session/new` takes `cwd` and `mcpServers` and returns a `sessionId` — [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- `session/load` needs the `loadSession` agent capability and takes `sessionId`, `cwd` (absolute) and `mcpServers`. The agent "MUST replay the entire conversation" as `user_message_chunk` and `agent_message_chunk` `session/update` notifications. Compactions are replayed only if the client supports `session.compaction`. — [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- `session/resume` needs `sessionCapabilities.resume` and reconnects without replay: the agent "MUST NOT replay the conversation history". `session/close` needs `sessionCapabilities.close`. — [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- The stable session-setup page does not mention `session/list` or `session/fork` — [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- The Claude adapter `@agentclientprotocol/claude-agent-acp` lists an experimental ACP v2 surface: `session/new`, `session/list`, `session/resume` (with replay), `session/close`, `session/prompt`, `session/cancel`, `session/delete` and `session/fork` — [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp)
- **Transport:** stdio is the only transport in the stable spec. The ACP site says "Full support for remote agents is a work in progress". A Transports working group formed in April 2026, and its Streamable HTTP and WebSocket RFD reportedly moved to Active on 2 July 2026, with a reference implementation underway in Goose. — [ACP intro](https://agentclientprotocol.com/get-started/introduction); [danilchenko.dev](https://www.danilchenko.dev/posts/agent-client-protocol/); [acpserver.org](https://acpserver.org/). Secondary sources disagree on dates: one reports a v2 draft on 20 July 2026, with remote transport still a draft in October 2026.
- **Ecosystem:** about 40–42 agents are in the ACP registry as of September 2026 (secondary sources). The community client `acp-ui` (desktop, mobile and web) reaches remote agents over ws:// and wss:// by wrapping a local stdio agent as a WebSocket endpoint, and lists Claude, Codex, Copilot, Qwen, Gemini, OpenCode and OpenClaw as compatible — [acp-ui](https://github.com/formulahendry/acp-ui); [danilchenko.dev](https://www.danilchenko.dev/posts/agent-client-protocol/)
- The adapter "implements an ACP agent by using the official Claude Agent SDK" and supports client MCP servers and permission requests — [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp)

**ACP: Agent Communication Protocol (IBM/BeeAI), now folded into A2A**
- On 29 August 2025, IBM and the Linux Foundation announced that ACP would merge into A2A. The ACP team wound down development, IBM's Kate Blair joined the A2A Technical Steering Committee, and BeeAI now uses A2A. The ACP repo is archived and a migration guide exists. — [LF AI & Data](https://lfaidata.foundation/?p=7689); [IBM Research](https://research.ibm.com/projects/agent-communication-protocol); [i-am-bee/acp](https://github.com/i-am-bee/acp)
- "ACP" also names Zed's Agent Client Protocol and the unrelated Agentic Commerce Protocol — [howaiworks.ai](https://howaiworks.ai/mcp-vs-a2a-vs-acp-how-ai-agents-talk/)

**MCP (Model Context Protocol), spec 2026-07-28**
- The 2026-07-28 spec retires the `initialize`/`initialized` handshake and the `Mcp-Session-Id` header. Each request carries its protocol version, client identity and capabilities in `_meta`, and an optional `server/discover` reports capabilities — [MCP blog 2026-07-28](https://blog.modelcontextprotocol.io/posts/2026-07-28/)
- State is now application-level: a server can "mint an explicit handle from a tool," which the model passes back in later calls. Requests can land on any instance behind a round-robin balancer without shared storage. — [MCP blog](https://blog.modelcontextprotocol.io/posts/2026-07-28/)
- Tasks moved into the `io.modelcontextprotocol/tasks` extension, with poll-based `tasks/get` and a new `tasks/update`. MCP Apps and Enterprise Managed Authorization are also extensions. Server-to-client requests use Multi Round-Trip Requests (MRTR) instead of held-open streams, and change notifications use `subscriptions/listen`. — [MCP blog](https://blog.modelcontextprotocol.io/posts/2026-07-28/); [4sysops](https://4sysops.com/archives/2026-07-28-model-context-protocol-mcp-stateless-multi-round-trip-routable-headers-authorization-hardening/)
- The release candidate came out 21 May 2026 and the final spec on 28 July 2026. The previous spec was November 2025. Roots, Sampling and Logging are deprecated but supported for at least 12 months, and legacy HTTP+SSE gets a one-year offramp. The footer credits "Model Context Protocol a Series of LF Projects, LLC". — [MCP blog](https://blog.modelcontextprotocol.io/posts/2026-07-28/); [hidekazu-konishi timeline](https://hidekazu-konishi.com/entry/mcp_specification_version_timeline.html)
- The post does not say whether handle-based state survives a change of host — [MCP blog](https://blog.modelcontextprotocol.io/posts/2026-07-28/)

**AG-UI (CopilotKit)**
- **Events:**
  - Lifecycle: RunStarted, RunFinished, RunError, StepStarted, StepFinished
  - Text: TextMessageStart, Content, End, Chunk
  - Tool calls: ToolCallStart, Args, End, Result, Chunk
  - State: StateSnapshot, StateDelta, MessagesSnapshot
  - Other: Activity events, Reasoning events, SubagentStarted, Finished and Error, Raw, Custom
  
  — [AG-UI events](https://docs.ag-ui.com/concepts/events)
- `threadId` identifies a conversation and `runId` one run. The optional `parentRunId` forms an append-only, branch-like history. — [AG-UI events](https://docs.ag-ui.com/concepts/events)
- StateSnapshot replaces the client's state. StateDelta carries RFC 6902 JSON Patch, and the client may request a fresh snapshot if it detects inconsistencies. MessagesSnapshot carries the full history and is useful for "synchronizing after connection interruptions". Interrupts are resumed by starting a new run with a `resume` array. — [AG-UI events](https://docs.ag-ui.com/concepts/events)

**Agent Protocol (langchain-ai) and LangGraph threads/runs**
- The spec is framework-agnostic and MIT-licensed. LangGraph Platform implements a superset. Its resources are Runs, Threads and a Store for long-term memory. — [agent-protocol repo](https://github.com/langchain-ai/agent-protocol)
- **Thread endpoints:** create, search, update and delete threads. `GET /threads/{id}/history` browses state revisions, and `POST /threads/{id}/copy` makes an independent copy. — [agent-protocol repo](https://github.com/langchain-ai/agent-protocol)
- **Background runs:** `POST /runs`, `GET /runs/{id}`, `/wait`, cancel. `GET /runs/{run_id}/stream` joins an existing run's stream, but only for output produced after the join. — [agent-protocol repo](https://github.com/langchain-ai/agent-protocol)
- **Thread streaming:** `POST /threads/{id}/stream` uses SSE, `GET /threads/{id}/stream` upgrades to WebSocket, and `POST /threads/{id}/commands` sends commands. Replay from an event-id is on the roadmap. — [agent-protocol repo](https://github.com/langchain-ai/agent-protocol)

**OpenAI Responses / Conversations API and the Agents SDK Session**
- The Conversations API persists conversation state as a durable object that can be reused "across sessions, devices, or jobs". `previous_response_id` chaining is the alternative, and the two cannot be combined — [OpenAI conversation state guide](https://developers.openai.com/docs/guides/conversation-state)
- The Agents SDK `Session` protocol has `get_items`, `add_items`, `pop_item` and `clear`, so any backend can store history. `OpenAIConversationsSession` stores it in the Conversations API. — [Agents SDK Python sessions](https://openai.github.io/openai-agents-python/sessions/); [Agents SDK JS sessions](https://openai.github.io/openai-agents-js/guides/sessions)

**Letta Agent File (.af)**
- **Serialized:** model config (context window, model and embedding names), message history with an `in_context` flag, system prompt, memory blocks, tool rules, environment variables, and tools with their source code and JSON schema — [letta-ai/agent-file](https://github.com/letta-ai/agent-file)
- **Not serialized:** archival-memory passages ("We currently do not support Passages"). Secrets are nulled on export. MCP server configs, files, multi-agent files, schema migration and cross-framework converters are on the roadmap. — [letta-ai/agent-file](https://github.com/letta-ai/agent-file)
- It is Apache-2.0 and maintained by Letta. It is imported and exported through the Letta API (`POST /v1/agents/import`, `GET /v1/agents/{id}/export`). The schema lives in `letta/serialize_schemas/pydantic_agent_schema.py`, and no format version number is published. — [letta-ai/agent-file](https://github.com/letta-ai/agent-file)

**OpenTelemetry GenAI semantic conventions**
- The GenAI conventions moved to a separate `semantic-conventions-genai` repo as of semconv v1.42.0. Most sources still label them "Development". A single June 2026 claim of stabilisation is unconfirmed. — [TrueFoundry](https://www.truefoundry.com/blog/opentelemetry-genai-semantic-conventions); [Greptime May 2026](https://www.greptime.com/blogs/2026-05-09-opentelemetry-genai-semantic-conventions); conflicting: [Business Tech Navigator](https://businesstechnavigator.com/news/opentelemetry-genai-semantic-conventions-agents-2026)
- Agent operations are create_agent, invoke_agent, invoke_workflow, plan and execute_tool, with attributes `gen_ai.agent.id`, `name` and `version`. Content capture is opt-in. — [Greptime](https://www.greptime.com/blogs/2026-05-09-opentelemetry-genai-semantic-conventions); [TrueFoundry](https://www.truefoundry.com/blog/opentelemetry-genai-semantic-conventions)

### Inferences
- **A2A:** at best an outer envelope for delegation. Agent A could start a Task on agent B and attach the transcript as a file or data Part, with an Extension declaring a "session-transfer" schema. Nothing in the spec gives that payload meaning, and the opacity principle points the other way. A2A also cannot attach a UI to an existing internal session.
- **Zed ACP:** the best existing fit for "detach a UI and attach it to another runtime" for coding agents. `session/load` (full replay) and `session/resume` (no replay) are exactly the reattach semantics. It still does not move state between agent instances: the agent must already have the session's state locally (for Claude Code, its transcript store), and `cwd` must be valid on that host. Cross-host use needs the transcript and workspace to be synced first, plus the not-yet-stable remote transport.
- **MCP:** going stateless means tool-server state is not part of the agent session. After a host move, MCP servers are re-established from config (ACP passes `mcpServers` on load and resume). Any state a server holds behind handles survives only if the server itself is remote and shared.
- **AG-UI and Agent Protocol:** good models for a client following a session wherever it runs. AG-UI's MessagesSnapshot plus StateSnapshot/Delta, and Agent Protocol's thread history, copy and stream-join, give snapshot-then-tail semantics. Agent Protocol's `threads/{id}/copy` is the closest standard primitive to forking or handing off a thread, though only within one server deployment.
- **Portable state as data:** OpenAI's Conversations object and the Agents SDK `Session` interface show the "history lives in a service, any runner can pick it up" pattern. Letta `.af` shows the "serialize everything to a file" pattern. Neither format is a cross-vendor standard.

### Gaps
- I did not read the full A2A spec (only the first 100k of 257k characters), so Part, Artifact and AgentCard field definitions and any official state-transfer extension are unverified. I found no evidence of an A2A extension for session or state transfer.
- I did not verify MCP Streamable HTTP resumability (`Last-Event-ID` on SSE streams in the 2025-era specs) against a primary source for 2026-07-28. The blog post does not mention it.
- I could not confirm the ACP v2 status or date, or whether `session/list`, `session/fork` and `session/delete` are stable or draft, from an official changelog. Sources conflict on dates.
- I could not establish the version history or date for Agent Protocol, or its link to the original AI Engineer Foundation "Agent Protocol" (an AutoGPT-era tasks/steps API). The repo page does not mention it.
- I did not fetch LangGraph checkpointer docs directly. My understanding is that thread checkpoints in Postgres/SQLite let any worker resume a thread, but that is from training knowledge, not a fetched source.
- I could not confirm a `gen_ai.conversation.id` attribute or any OTel "session export" convention. I found no emerging standard for a portable agent transcript format.

## Which protocols model state transfer vs task delegation vs UI attachment, and which layer fits (a) handing a session to a different agent instance and (b) letting a phone or laptop client follow a session wherever it runs?

### Takeaway
- **Task delegation:** A2A, plus MCP Tasks for long-running tool calls.
- **UI attachment and streaming:** Zed ACP for editor↔local agent, AG-UI for app UI↔agent backend, and Agent Protocol stream-join.
- **State persistence and transfer:** only data formats or services, not wire protocols: Letta `.af`, OpenAI Conversations, LangGraph threads and checkpoints, Agent Protocol thread copy.

For (a), the right layer is a state store or serialized transcript. For (b), it is a UI-attachment protocol with snapshot-plus-tail semantics, keyed by a stable session ID routed through a relay.

### Cited Findings
- A2A agents collaborate "without needing access to each other's internal state, memory, or tools", so A2A is delegation by design — [A2A spec](https://a2a-protocol.org/latest/specification/)
- ACP `session/load` replays the full conversation and `session/resume` reattaches without replay. Both reattach a client to an agent-held session. — [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- AG-UI MessagesSnapshot is useful for "synchronizing after connection interruptions". StateSnapshot/StateDelta keep client state in sync. — [AG-UI events](https://docs.ag-ui.com/concepts/events)
- Agent Protocol `GET /runs/{run_id}/stream` joins a running run, but only for subsequent output. A WebSocket thread stream exists, and event-id replay is on the roadmap. — [agent-protocol](https://github.com/langchain-ai/agent-protocol)
- OpenAI's Conversations object is reusable "across sessions, devices, or jobs" — [OpenAI guide](https://developers.openai.com/docs/guides/conversation-state)
- Letta `.af` serializes message history, memory blocks, tools and config for re-import into another Letta server — [agent-file](https://github.com/letta-ai/agent-file)
- MCP 2026-07-28 is stateless at the protocol level, with no session ID — [MCP blog](https://blog.modelcontextprotocol.io/posts/2026-07-28/)

### Inferences

**(a) Handing a session to a different agent instance**

Proposed design, not an existing protocol:
1. Serialize the transcript, compaction summaries, todo and plan state, and the permission mode.
2. Sync the workspace separately (git commit or worktree plus uncommitted diff), because no agent protocol carries the filesystem.
3. Start B with the transcript in its local store.
4. Have B's UI reattach through ACP `session/load` (or Claude Code's `--resume`).

A2A could wrap this as a "continue this context" task, but would need a custom extension.

**(b) A client following a session wherever it runs**

1. The client subscribes by a stable session ID to a relay.
2. It receives a full snapshot (ACP load replay, AG-UI MessagesSnapshot) followed by incremental events.
3. When the session moves hosts, the relay rebinds the ID to the new runtime, and the client gets a resync snapshot.

Claude Code Remote Control already uses a relay, a server-stored transcript and multi-device sync for one host (see the next section).

**Failure modes and known gaps**
- In-flight tool calls and open permission prompts have no standard representation for transfer. A handoff should happen only at a turn boundary (idle or completed state), similar to A2A's terminal and interrupted states.

### Gaps
- I found no protocol or spec that explicitly defines "live migration" of an agent session between hosts.

## Real-world implementations relevant to Claude Code

### Takeaway
Claude Code already has proprietary mechanisms for both halves. Remote Control is an outbound-HTTPS relay through the Anthropic API that keeps execution local and lets claude.ai/code and the mobile app follow and steer the session. `/teleport` and `--teleport` pull a cloud session into a local terminal. On the open-standard side, `claude-agent-acp` exposes Claude via ACP, including experimental `session/list`, `resume`, `fork` and `delete`.

### Cited Findings
- **Where Remote Control runs:** "Claude keeps running locally the entire time". The web and mobile UIs are "a window into that local session". The machine must stay on. — [Claude Code Remote Control docs](https://code.claude.com/docs/en/remote-control)
- **Transport:** the session "makes outbound HTTPS requests only and never opens inbound ports". It registers with the Anthropic API and polls for work, and messages are routed "over a streaming connection". Traffic uses TLS with multiple short-lived, purpose-scoped credentials. — [Remote Control docs](https://code.claude.com/docs/en/remote-control)
- **Stored transcript:** while Remote Control is connected, the transcript (including messages and tool activity) is stored on Anthropic servers to sync devices and reconnect after network drops. It is unavailable under Zero Data Retention or HIPAA configurations. — [Remote Control docs](https://code.claude.com/docs/en/remote-control)
- **Multi-device sync:** the conversation, subagents and workflows stay in sync across all connected devices, and compaction and `/clear` propagate. Sessions can be forked from claude.ai/code (requires v2.1.277 or later). `claude remote-control` server mode runs multiple sessions per process, with a `--spawn worktree` option. — [Remote Control docs](https://code.claude.com/docs/en/remote-control)
- **Teleport:** when a cloud session is pulled into the terminal with `/teleport`, connected devices do not receive the earlier history, only new messages. Cross-session messaging carries messages between sessions on different machines over the same connection. — [Remote Control docs](https://code.claude.com/docs/en/remote-control)
- Remote Control is unavailable with Bedrock, Vertex/Agent Platform, Foundry, a custom `ANTHROPIC_BASE_URL` or API keys — [Remote Control docs](https://code.claude.com/docs/en/remote-control)
- Third-party guides describe `claude --teleport <session-id>` as pulling a web or cloud session locally, requiring a clean git tree and the same claude.ai account. One August 2026 blog says the flag is missing from `--help` in its version, so sources conflict. — [codeongrass](https://codeongrass.com/blog/claude-code-teleport-hand-off-phone/); [wmedia.es](https://wmedia.es/en/tips/claude-code-teleport-not-remote-control)
- A community feature request asks for "Universal session teleporting (local → remote, SSH support)", which suggests that arbitrary host-to-host transfer is not built in — [claudeissues #14666](https://claudeissues.com/issue/14666-feature-universal-session-teleporting-local-remote-ssh-support)
- `claude-agent-acp` builds on the Claude Agent SDK and supports MCP and permissions, plus experimental v2 session methods — [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp)

### Inferences
- Remote Control already implements requirement (b) for one fixed host. Extending it to follow a session across hosts would mean rebinding the server-side session ID to a new runtime. The stored transcript would serve as the transfer payload, and the workspace would move through git.
- For an open, self-hosted equivalent, the closest stack is:
  - ACP (client↔agent: load, resume, fork)
  - a WebSocket relay like `acp-ui`'s
  - transcript sync between hosts
  - git or worktree sync for the filesystem

### Gaps
- The adapter README does not say how ACP session IDs map to Claude Agent SDK session IDs or on-disk JSONL transcripts, or whether `session/load` works on a host other than the one that created the session.
- The official `--teleport` docs were not fetched directly. Only the Remote Control page's reference to `/teleport` was verified.

## Papers and specs on agent migration, context handoff and checkpoint/restore

### Takeaway
2026 arXiv work focuses on single-host checkpoint/restore and its pitfalls: replayed side effects, environment state, and the semantics and security of resume. I found no paper specifically on cross-host migration of LLM agent sessions. The main lessons for handoff design are to capture environment state (not just context) and to avoid re-executing side effects.

### Cited Findings
- **AgentRewind** (arXiv 2608.14380) records recoverable checkpoints at each LLM decision point, capturing agent context and controlled environment state, and injects "rewind memory" — [arXiv](https://arxiv.org/html/2608.14380)
- **Crab** (arXiv 2604.28138) is a semantics-aware checkpoint/restore runtime for agent sandboxes. It captures OS-level state and returns cached responses to replaying agents so side effects are not repeated — [arXiv](https://arxiv.org/html/2604.28138v1)
- **Execution-State Capsules** (arXiv 2606.20537) treat a coding agent's warm start as snapshot/restore and argue that restoring only the KV cache diverges from a full-state restore. Restores are gated by a validity check. — [arXiv](https://arxiv.org/html/2606.20537v1)
- **Resume Means Resume** (arXiv 2608.03836) is a machine-checked conformance contract for checkpoint, interrupt and resume. It notes that LangGraph, LlamaIndex Workflows and CrewAI persist run state with differing semantics. — [arXiv](https://arxiv.org/html/2608.03836v2)
- **Safe to Resume?** (arXiv 2608.29381) finds that restored checkpoints can invalidate state, context or external effects. It recommends deterministic state references. — [arXiv](https://arxiv.org/pdf/2608.29381v1)
- **"When Can Agents Safely Checkpoint, Fork, Restore, and Merge?"** (arXiv 2608.22928) covers exact checking for execution edits — [arXiv](https://arxiv.org/pdf/2608.22928)
- **Surveys:** "A Survey of AI Agent Protocols" (arXiv 2504.16736) — [arXiv](https://arxiv.org/pdf/2504.16736). "A2ABreak" (arXiv 2609.10871) is a systematic security analysis of A2A — [arXiv](https://arxiv.org/pdf/2609.10871)

### Inferences
- A cross-host handoff is a checkpoint on host 1 plus a restore on host 2. The literature suggests the payload must include the environment (workspace, running processes and their side effects), not just the transcript. The restore should be validated against the target environment, for example that the git HEAD matches and the `cwd` exists.

### Gaps
- I only read abstracts and snippets of these papers, not their full text. I found no academic work on migrating an interactive coding-agent session across machines while a UI stays attached.
