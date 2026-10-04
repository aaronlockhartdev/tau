# Implementing ACP in tau — research

**Scope.** What implementing the Agent Client Protocol (ACP) in tau entails, and the
most optimal architecture for a headless `tau acp` subcommand that speaks ACP over
stdio, so external ACP clients (Harbor's eval framework, IDEs) can drive tau, and
tau can be listed in the ACP registry.

**Date.** 2026-10. **Spec baseline.** ACP v1 (stable, schema 1.10.2). v2 is
unstable and out of scope for v0 (see §2.2).

---

## 1. Executive summary

**Recommendation in one paragraph.** Add a new workspace crate `crates/tau-acp`
that contains the entire ACP server as a library plus a thin `tau-acp` binary,
and branch to it at the very top of `app/src-tauri/src/main.rs` when the first
argument is `acp` (before any Tauri builder is constructed). The server builds a
production-shape `Core` exactly the way the eval rig does
(`CoreBuilder::default_system().build()` over the user's real
`~/.config/tau`), drives it through the app's own dispatch protocol
(`WorkspaceOpen` → `SessionNew` → `MessageSend`), and streams to the ACP client
by taking the core's **existing** single-tap event channel, `Core::events()`,
with a dedicated pump task that converts the core's frame-aligned (≤16 ms
cadence, ADR-0008) `EntryUpsert` snapshots into incremental ACP
`session/update` notifications. No tau-core changes are required for v0: the
16 ms snapshot cadence makes the bounded 1024-slot pipe effectively
non-dropping for a dedicated consumer, and the append-only session journal
provides a self-healing resync source if a drop is ever detected (the pipe
already counts drops). Turn completion is the eval rig's proven settle rule
(first `StreamEnd`, then an empty `Queue` event, or a `System` error). v0
advertisses **one `agent`-type auth method** (OpenAI-compatible endpoint,
credential passed in-band via `authenticate`'s `_meta`, the codex-acp
pattern) plus env-var overrides (`TAU_BASE_URL`/`TAU_API_KEY`/`TAU_MODEL`) for
Harbor, `session/set_config_option` as the model selector (Harbor's required
model-selection mechanism), and **no** `session/request_permission` at all —
tau v0 has no approval flow by design (ADR-0007), which matches Harbor's
default auto-allow behavior. The registry entry ships the thin `tau-acp`
musl-static binary (the Tauri app binary cannot be static — wry needs system
webkit2gtk — so distribution and the in-app subcommand share the same library
but ship as two binaries).

**Minimal v0 method set** (passes registry CI verification and runs a
Terminal-Bench-style task through Harbor):

| direction | method | notes |
|---|---|---|
| client→agent | `initialize` | respond `protocolVersion: 1`, capabilities, `authMethods` (≥1 `agent`/`terminal` — CI-enforced) |
| client→agent | `authenticate` | no-op when config already has credentials; `_meta`-carried endpoint/key otherwise |
| client→agent | `session/new` | `cwd` → `WorkspaceOpen` + `SessionNew`; respond `sessionId` + model `configOptions` |
| client→agent | `session/prompt` | text blocks → `MessageSend`; stream `session/update`; respond `{stopReason}` on settle |
| client→agent | `session/cancel` | → `MessageStop`; respond the in-flight prompt with `cancelled` |
| client→agent | `session/set_config_option` | model select → `SessionSetModel` (Harbor `--model` path) |
| agent→client | `session/update` | `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, (optional `usage_update`) |
| agent→client | (none of) `session/requestPermission`, `fs/*`, `terminal/*` | v0: no HITL (ADR-0007); tau owns its filesystem directly |

Deferred: `session/load` (trivial later — journal replay is last-per-id by
construction), `logout`, plans, modes, MCP servers (accepted, ignored — v0 has
no MCP, ADR-0003).

---

## 2. ACP protocol reference (v1)

Primary source: <https://github.com/agentclientprotocol/agent-client-protocol>
(docs in `docs/protocol/v1/`, machine schema in `schema/v1/schema.json`). All
JSON shapes below were extracted from that schema, not paraphrased.

### 2.1 Transport & framing

- JSON-RPC 2.0 over **stdio**: the client launches the agent as a subprocess;
  the agent reads JSON-RPC messages from stdin and writes them to stdout.
  Messages are **newline-delimited** (`\n`) and **MUST NOT contain embedded
  newlines** (UTF-8).
- The agent **MAY** write UTF-8 logs to stderr; it **MUST NOT** write anything
  to stdout that is not a valid ACP message. (⇒ all of tau's logging in ACP
  mode must go to stderr.)
- Streamable HTTP is a draft; custom transports are allowed but irrelevant
  here. (`docs/protocol/v1/transports.mdx`)

### 2.2 Versioning & negotiation

- `protocolVersion` is a **single integer MAJOR version**. Current stable:
  **1** (crate/schema 1.10.2, 2026-10-01). **v2 is unstable** (feature-gated
  in all SDKs; changes `authenticate`→`auth/login`+`auth/logout` and the prompt
  lifecycle to accept-then-idle). The registry's CI probes with
  `protocolVersion: 1` and the nightly protocol matrix runs v1.
- Negotiation: the client sends the latest version it supports; the agent
  responds with the same version if supported, else its own latest; the client
  should disconnect on mismatch. New features land as **capabilities**, not
  version bumps.

### 2.3 `initialize` (exact shapes)

Request (client→agent):

```json
{
  "jsonrpc": "2.0", "id": 0, "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {
      "fs": { "readTextFile": true, "writeTextFile": true },
      "terminal": true,
      "auth": { "terminal": false }
    },
    "clientInfo": { "name": "my-client", "title": "My Client", "version": "1.0.0" }
  }
}
```

(`protocolVersion` is the only required field; `clientCapabilities` and
`clientInfo` default/are nullable on decode.)

Response (agent→client):

```json
{
  "jsonrpc": "2.0", "id": 0,
  "result": {
    "protocolVersion": 1,
    "agentCapabilities": {
      "loadSession": false,
      "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
      "mcpCapabilities": { "http": false, "sse": false },
      "sessionCapabilities": {},
      "auth": {}
    },
    "authMethods": [ /* see §2.4 */ ],
    "agentInfo": { "name": "tau", "title": "Tau", "version": "0.1.0" }
  }
}
```

Agent capability fields (all optional, omitted = unsupported):
`loadSession: bool` (default false); `promptCapabilities.{image,audio,embeddedContext}`
(baseline: all agents **MUST** accept `text` and `resource_link` content in
prompts); `mcpCapabilities.{http,sse}`; `sessionCapabilities.{delete,
additionalDirectories, list, resume, close}`; `auth.logout: {}`.

### 2.4 `authMethods` (registry CI-verifies these)

`AuthMethod` is a discriminated union on `type`:

- **`agent`** (the default when `type` is absent):
  `{ "id", "name", "description?", "_meta?" }` — the agent handles
  authentication itself through the `authenticate` method.
- **`terminal`**:
  `{ "id", "name", "description?", "args": string[], "env": {k: v} }` — the
  client re-launches the *configured agent program* (its own launch command)
  with appended `args`/`env`, presents the interactive terminal, waits for
  exit 0, then reconnects. The client **MUST NOT** pass a terminal method to
  `authenticate`. Terminal methods may only be advertised when the client
  advertised `clientCapabilities.auth.terminal: true`. (RFD:
  `docs/rfds/auth-methods.mdx`.)

**What registry CI actually verifies** (registry repo,
`.github/workflows/client.py` + `verify_agents.py`): the CI spawns the agent
in an **isolated sandbox** (fresh temp `HOME`, sanitized env — only a fixed
allowlist of vars passes through, so no `TAU_*`/provider keys are present),
sends `initialize` (with `clientCapabilities` including `terminal: true`,
`fs` read/write, and `_meta: {"terminal-auth": true}`), and requires the
response to contain **at least one authMethod whose type is `agent` or
`terminal`** (`validate_auth_methods`: "No authMethods in response" is a
failure). Type detection: explicit `type` field, else `_meta["terminal-auth"]`
/`_meta["agent-auth"]`, else default `agent`. Consequence for tau: `tau acp`
**must advertise ≥1 auth method even with an empty config** — i.e. the
`authMethods` list is static, not conditional on configured credentials.

The registry's nightly **protocol matrix** (`protocol_matrix.py`) additionally
launches each registered agent and probes `initialize` + `session/new`
(unauthenticated) plus a set of unstable methods — so `session/new` must
succeed (or at least fail cleanly) with no auth.

`authenticate` (client→agent): `params: { "methodId": <adverted id> }`,
`result: {}`. `logout` (client→agent, requires `agentCapabilities.auth.logout`):
`params: {}`, `result: {}`.

**Precedent for in-band credentials** (used by codex-acp): the `authenticate`
request may carry the actual credential in `_meta` — e.g. codex-acp's
`api-key` method reads `_meta["api-key"].apiKey`. `_meta` is the spec's
blessed extension channel ("Implementations MUST NOT make assumptions about
values at these keys" — i.e. agents define and document their own `_meta`
extensions).

### 2.5 Session lifecycle

**`session/new`** (client→agent):

```json
{ "jsonrpc":"2.0", "id":1, "method":"session/new",
  "params": { "cwd": "/abs/path", "mcpServers": [ /* optional; stdio/http/sse server specs */ ] } }
```

(`cwd` + `mcpServers` are the required fields; `cwd` **MUST** be absolute.)
Response:

```json
{ "jsonrpc":"2.0", "id":1,
  "result": { "sessionId": "sess_abc123", "modes": null, "configOptions": [ /* optional */ ] } }
```

**`session/prompt`** (client→agent):

```json
{ "jsonrpc":"2.0", "id":2, "method":"session/prompt",
  "params": { "sessionId": "sess_abc123",
    "prompt": [ { "type": "text", "text": "…" }, { "type": "resource_link", "uri": "file:///…", "name": "…" } ] } }
```

Response — sent only when the **turn ends**:

```json
{ "jsonrpc":"2.0", "id":2, "result": { "stopReason": "end_turn" } }
```

**`session/cancel`** (client→agent, **notification**):
`params: { "sessionId": … }`. On receipt the agent should stop all work and
**MUST** answer the original `session/prompt` with `stopReason: "cancelled"`
(never an error — clients display agent errors to users).

**`session/load`** (optional, requires `loadSession: true`): replay the whole
conversation as `session/update` notifications, then respond `{}`.

**`session/set_config_option`** (stable v1):
`params: { "sessionId", "configId", "value"? }`. This is how clients select
the model when the agent advertises a `model`-category select option in
`session/new`'s `configOptions` (see §2.7).

### 2.6 `session/update` — the full v1 variant set

`params: { "sessionId": …, "update": { "sessionUpdate": <variant>, … } }`.
The complete discriminator set in current v1 (schema `SessionUpdate`):

| `sessionUpdate` | payload (beyond the discriminator) |
|---|---|
| `user_message_chunk` | `content: ContentBlock`, `messageId?` |
| `agent_message_chunk` | `content: ContentBlock`, `messageId?` |
| `agent_thought_chunk` | `content: ContentBlock`, `messageId?` |
| `tool_call` | `toolCallId`, `title` (required); `name?`, `kind?`, `status?` (default `pending`), `content?`, `locations?`, `rawInput?`, `rawOutput?` |
| `tool_call_update` | `toolCallId` (required); all other fields optional — only changed fields are sent; `null`/omission leaves prior values unchanged |
| `plan` | `entries: [{ content, priority: high\|medium\|low, status: pending\|in_progress\|completed }]` |
| `available_commands_update` | `commands: [{ name, description, input? }]` |
| `current_mode_update` | `currentModeId`, `availableModes` |
| `config_option_update` | `configOptions: ConfigOption[]` |
| `session_info_update` | `title?`, `updatedAt?` |
| `usage_update` | `used` (int, required), `size` (int, required), `cost? {amount, currency}` |

Message chunks: `content` is a single `ContentBlock`; chunks sharing a
`messageId` belong to one message; a changed `messageId` starts a new message.
(`messageId` is optional — agents MAY omit it.)

Example tool call create + complete:

```json
{ "jsonrpc":"2.0", "method":"session/update",
  "params": { "sessionId":"s1", "update": {
    "sessionUpdate": "tool_call", "toolCallId": "call_001",
    "name": "bash", "title": "Run tests", "kind": "execute",
    "status": "in_progress", "rawInput": { "command": "cargo test" } } } }

{ "jsonrpc":"2.0", "method":"session/update",
  "params": { "sessionId":"s1", "update": {
    "sessionUpdate": "tool_call_update", "toolCallId": "call_001",
    "status": "completed",
    "content": [ { "type": "content", "content": { "type": "text", "text": "…output…" } } ],
    "rawOutput": "…output…" } } }
```

### 2.7 `ConfigOption` (model selection)

In `session/new`'s `configOptions`, a model selector is:

```json
{ "id": "model", "name": "Model", "category": "model", "type": "select",
  "currentValue": "gpt-5", "options": [ { "value": "gpt-5", "name": "GPT-5" } ] }
```

(`ConfigOption` = `{id, name, description?, category?}` + one of
`{type:"select", currentValue, options:[{value,name,description?}](,groups?)}`
or `{type:"boolean", currentValue}`.) Changing it goes through
`session/set_config_option` with `configId: "model"`.

### 2.8 Agent→client request methods (v0: all unused by tau)

- **`session/request_permission`**: `params: { sessionId, toolCall: ToolCallUpdate, options: PermissionOption[] }`;
  `PermissionOption = { optionId, name, kind }` with
  `kind ∈ allow_once | allow_always | reject_once | reject_always`;
  response `result: { outcome }` where outcome is
  `{outcome:"cancelled"}` or `{outcome:"selected", optionId}`. Clients MAY
  auto-allow/reject per their settings.
- **`fs/read_text_file`**: `{sessionId, path, line?, limit?}` → `{content}`.
- **`fs/write_text_file`**: `{sessionId, path, content}` → `{}`.
- **`terminal/create|output|wait_for_exit|kill|release`** (require the client's
  `terminal` capability): create a client-side shell process for the agent to
  drive; tool calls can embed `{type:"terminal", terminalId}` content.
- **`elicitation/create`** (form/url modes) — v1 addition.

All file paths in the protocol MUST be absolute; line numbers are 1-based.
JSON object keys are `camelCase`; discriminator string values are `snake_case`.

### 2.9 Content blocks, stop reasons, tool kinds

`ContentBlock` (same shape as MCP 2025-06-18):

- `{type:"text", text}` — **must** be supported in prompts by all agents
- `{type:"resource_link", uri, name, title?, description?, mimeType?, size?}` — **must** be supported
- `{type:"image", data (b64), mimeType, uri?}` — requires `image` prompt capability
- `{type:"audio", data (b64), mimeType}` — requires `audio`
- `{type:"resource", resource:{uri, text|blob, mimeType?}}` — requires `embeddedContext`

`ToolCallStatus`: `pending | in_progress | completed | failed`
(note: **no `cancelled` status** in v1 — a cancelled tool is simply left
unfinalized; the client marks it locally on `session/cancel`).

`ToolKind`: `read | edit | delete | move | search | execute | think | fetch |
switch_mode | other` (default `other`).

`StopReason`: `end_turn | max_tokens | max_turn_requests | refusal | cancelled`.
(There is **no `error` stop reason** — a failed turn is a JSON-RPC error
response to `session/prompt`, not a stop reason. pi-acp's `'error'` value is a
non-standard extension.)

`ToolCallContent`: `{type:"content", content: ContentBlock} |
{type:"diff", path, oldText?, newText} | {type:"terminal", terminalId}`.

### 2.10 Conformance testing

`agentclientprotocol/acp-tck` (experimental) launches an agent as a stdio
subprocess and drives initialize/session/prompt/cancel/error/transport
hygiene, reporting pass/fail per spec requirement. Use it as the
interoperability gate.

---

## 3. Implementation architecture for `tau acp`

### 3.1 Entry-point seam

`app/src-tauri/src/main.rs` today is one flat `fn main()` that builds the
`tauri::Builder` (menu, `CoreState(CoreBuilder::default_system().build())`,
pump spawn in `setup`, `builder.run(generate_context!())`). The cheapest clean
seam is a branch **before any Tauri code runs**:

```rust
fn main() {
    if std::env::args().any(|a| a == "acp") {
        tau_acp::run();            // headless stdio loop; never returns
        return;
    }
    // … existing tauri builder …
}
```

**But the distribution artifact must be a separate thin binary.** The Tauri
app links wry → system `libwebkit2gtk` on Linux, so it can never be the
static-musl binary the registry `binary` distribution wants. Therefore:

- new workspace crate **`crates/tau-acp`**: `[lib]` (the whole ACP server:
  transport loop, session registry, event→ACP mapper) + `[[bin]] tau-acp`
  (main: parse `acp`-less argv, `tokio::runtime().block_on(run())`).
  Dependencies: `tau-core`, `tau-protocol`, `tokio`, `serde_json` — and
  either the official `agent-client-protocol` Rust SDK or a ~200-line
  hand-rolled JSON-RPC dispatcher (see §3.6). No Tauri.
- `app/src-tauri` adds `tau-acp` as a dependency for the `tau acp` branch, so
  IDE users can register the *GUI binary* as their ACP agent while the
  registry ships the thin `tau-acp` binary. One library, two binaries, zero
  duplicated protocol logic.

Registry entry shape (see §4.5) then uses `cmd: "./tau-acp"` with no args.

### 3.2 Core driving (reuse the eval rig)

`crates/tau-eval/src/live.rs` already builds and drives a production-shape
core; `tau acp` reuses the same surface:

```rust
let core = tau_core::harness::CoreBuilder::default_system().build(); // ~/.config/tau
let ws = core.dispatch(Command::WorkspaceOpen { cwd });       // → workspace id
let s  = core.dispatch(Command::SessionNew { workspace, title: None }); // → SessionMeta.id
core.dispatch(Command::MessageSend { session, text, lane: MessageLane::FollowUp });
// cancel: Command::MessageStop { session }
// model:  Command::SessionSetModel { session, model }
```

Notes:

- `Core::dispatch` is **synchronous** (`pub fn dispatch(self: &Arc<Self>, …) -> Result<CommandOutput, ProtocolError>`, mutex-guarded internally) — safe to call from any ACP task; the long-running work (the turn) proceeds in the core's own spawned task, observable through the event pipe.
- The ACP `sessionId` is the tau session id verbatim (`SessionMeta.id`,
  e.g. adjective-noun + monotonic entry ids live in the journal). `cwd` comes
  from `session/new`; the workspace id is derived by the core (xxh3 of cwd) —
  the ACP layer keeps a `sessionId → (workspaceId)` map.
- MCP servers in `session/new`: accepted and **ignored** (v0 has no MCP;
  ADR-0003 is OpenAI-compatible-only). Don't advertise `mcpCapabilities`.
- `session/load` (later): journal replay is cheap — read
  `{cwd}/.tau/sessions/{id}.jsonl`, keep last line per entry id, order by
  creation id (ADR-0008's "last-per-id" invariant), emit one
  `session/update` per entry. Defer past v0; don't advertise `loadSession`.

### 3.3 Event delivery (the critical design question)

**What exists in tau-core today** (verified in code):

1. One bounded event pipe: `Core { events_tx: mpsc::Sender<Event> }` with
   capacity **1024** (`harness/state.rs`, `CoreBuilder::build`). Every emit
   goes through `pipe_send`, which `try_send`s and **drops on overflow**,
   counting drops in `PipeCounters` and delivering a one-shot "event pipe
   overflow: N event(s) dropped" `System` error on the next successful send.
2. A **single-tap consumer**: `Core::events()` takes the one
   `mpsc::Receiver` exactly once ("the event pump takes the receiver exactly
   once"). The GUI uses it via `harness::pump::pump` (a plain forwarder into
   Tauri `emit`). The eval rig uses it the same way and fans out to
   per-session *unbounded* channels (`Live::subscribe`).
3. **What the pipe actually carries during a turn** (ADR-0008, verified in
   `agent/turn.rs` + `harness/forwarding.rs`): streaming text deltas do **not**
   travel as per-token channel events. `StreamEntrySink` coalesces tokens into
   a ~16 ms window (`SNAPSHOT_INTERVAL = 16ms`) and re-emits the **whole
   growing entry** as `Event::EntryUpsert` (wire-only; the file line lands
   once, at call end). Tool calls: one `EntryUpsert` at call start (empty
   output) and one at result. Plus `StreamStart`/`StreamEnd{usage,
   interrupted}` per provider call, `Queue` snapshots, `System` errors.
4. The **journal** (`{cwd}/.tau/sessions/{id}.jsonl`) is append-only, one
   finalized line per entry id, written by the session store — the eval rig
   tails it on a 2 s poll because the pipe drops under load.

**Options:**

- **(a) Journal tailing** (eval rig). Lossless, zero core changes, but
  granularity is *per finalized entry*: an assistant message appears only when
  its provider call completes (can be tens of seconds into a long stream),
  and tool calls only at result time. **Not acceptable** for live
  `agent_message_chunk` streaming to an ACP client.
- **(b) Dedicated pump over the existing `Core::events()` tap.** The ACP
  server takes the receiver (it's the only consumer in that process) and a
  dedicated tokio task maps each `Event` to ACP notifications and writes
  them to stdout. At ≤16 ms snapshot cadence the event rate is ≤60
  events/s/stream (frame-capped by design); a consumer whose only job is
  clone→map→serialize→write is microseconds per event, and the 1024-slot
  buffer is then ~15 s of headroom. Drops become a theoretical case (client
  stops reading stdout for many seconds), and when they happen the pipe's
  drop counter already knows — the server can **resync from the journal**
  (re-read the session file, last-per-id, re-emit the affected entries as
  fresh `agent_message_chunk`/`tool_call_update` notifications). This is
  exactly ADR-0008's self-healing resync semantics applied to a wire client.
  **No tau-core change.**
- **(c) Add a lossless headless subscription seam to tau-core.** E.g. a second
  `mpsc::UnboundedSender<Event>` in `Core` fanned out in `emit` (and in the
  forwarding hooks), exposed as `Core::subscribe()` — strictly lossless at the
  cost of unbounded memory if a consumer stalls (a runaway stream with a dead
  client grows the buffer without bound — precisely the failure the bounded
  pipe guards against). This is the "purist" fix and is a small,
  well-motivated core change, but it buys nothing v0 can't get from (b) +
  journal resync.

**Recommendation: (b) + journal resync repair.** Zero core changes for v0;
losslessness in practice; a measured, bounded repair path for the residual
case. If post-v0 telemetry shows real drop rates, promote to (c) with the
drop counter as the trigger. The mapper design (which is identical under all
three options) is §3.4.

### 3.4 Event → ACP mapping

Per session, the ACP layer keeps: last-sent text/reasoning length per entry
id, the in-flight assistant entry ids (FIFO — provider calls are strictly
sequential per session, single-writer loop), and the pending
`session/prompt` response future.

| tau event | ACP output |
|---|---|
| `EntryUpsert` kind `assistant` (in-flight snapshot) | diff `payload.text` against last-sent length → `agent_message_chunk` `{content:{type:"text",text:delta}, messageId: entry.id}`; same for `payload.reasoning` → `agent_thought_chunk` |
| `StreamEnd{call_id, usage, interrupted}` | finalize the oldest in-flight assistant entry (its snapshot is final — `calls`/`usage` populated); if `usage` + model `context_window` known → `usage_update`; per-call, not per-turn |
| `EntryUpsert` kind `tool`, `output` empty | `tool_call` `{toolCallId: entry.id, name, title: derived, kind: mapped, status:"in_progress", rawInput: args, locations?}` |
| `EntryUpsert` kind `tool`, `output` present | `tool_call_update` `{toolCallId, status:"completed", content:[{type:"content",content:{type:"text",text:output}}], rawOutput: output}` (tool errored → `status:"failed"`) |
| `Queue` empty **after** first `StreamEnd` (turn settle, eval-rig rule) | respond the pending `session/prompt` with `stopReason:"end_turn"` |
| `System` error (session-scoped) at settle | JSON-RPC **error** response to `session/prompt` (no `error` stop reason exists in v1) |
| `session/cancel` | dispatch `MessageStop`; when settle fires → respond prompt with `stopReason:"cancelled"` (MUST be the response, even if the abort surfaced as an exception — spec is explicit) |
| (later) `SubagentEvent`/`TaskChanged` | ignore in v0 (or `agent_thought_chunk`; no client demand yet) |

Details:

- `messageId` = tau entry id (stable, unique per session, core-minted) —
  free, and gives clients correct message grouping.
- **Snapshot diffing** is the only stateful piece: `sent = text[len..]` per
  entry id, reset when the entry finalizes. Because wire snapshots and file
  lines share one format (ADR-0008), a journal resync can rebuild the same
  state exactly.
- Tool **title**: derive from args (`bash` → first ~80 chars of the command;
  `read`/`write`/`edit` → the path). `locations`: `{path}` from the path arg
  for file tools (cheap, and powers "follow-along" in IDEs).
- `ToolKind` map: `read→read`, `write→edit`, `edit→edit`, `bash→execute`,
  `recall→search`, `subagent_*→think`, `task_*→think`, everything else
  `other`.

### 3.5 Permissions (v0: none — by design)

tau v0 has **no approval flow** (ADR-0007 "v0 security model: transparency,
not enforcement": no permission popups, no per-tool policy; the protocol
keeps the HITL shape with zero live request types). So `tau acp` v0:

- **never calls `session/request_permission`** — tools execute immediately,
  which is exactly what Harbor's runner does by default
  (`HARBOR_ACP_PERMISSION_MODE` defaults to `allow`; its
  `request_permission` handler picks the first `allow_once`/`allow_always`
  option). An agent that never asks is fully compatible.
- No config surface for permission modes in v0 (no speculative code); the
  post-v0 security ticket (per-tool allow/deny policy is its first candidate)
  is where a real `requestPermission` flow would land, and the ACP layer
  already has the shape it needs.

### 3.6 Transport & dispatch implementation

Two viable options:

1. **Official Rust SDK** `agent-client-protocol` (crates.io, v2.2.0; v1 types
   stable, v2 behind `unstable_protocol_v2`). Provides the typed v1
   request/response types (generated from the same schema), the
   `Agent`/`Client` traits, `ByteStreams` stdio transport, and the
   `Agent::builder().on_receive_request(handler, on_receive_request!())`
   registration pattern. sigit (Rust, registry-listed) is production proof.
   **Caveat sigit documents (steal this lesson):** turn-affecting handlers
   must be *spawned*, serialized by a per-session turn lock — a handler that
   awaits a client request (e.g. a permission round-trip) **deadlocks** the
   dispatch loop, which is the only thing that can route the client's
   response. tau's v0 has no agent→client requests, so a plain spawned
   handler per `session/prompt` is safe, but the turn-lock pattern should be
   baked in now for the post-v0 permission flow.
2. **Hand-rolled** JSON-RPC 2.0 dispatcher: a stdin line reader (tokio
   `AsyncBufRead`), `serde_json` parse, match on `method`, write
   newline-terminated JSON to a locked stdout writer. ~200 lines, no dep,
   full control (and the protocol is small — 6 methods + 1 notification in
   v0 scope). Downside: hand-maintained types instead of schema-generated.

Recommendation: **start hand-rolled** — the v0 method set is tiny, hand
types keep the crate dep-free, and the mapping layer (§3.4) is where all the
real logic lives. Revisit the SDK when `session/load`, permissions, and
config options accumulate. (The SDK remains the fallback if TCK results show
we're drifting from reference behavior.)

**Concurrency model.** One tokio multi-thread runtime. Tasks: (1) stdin
reader → dispatch (initialize/session-new/session-prompt/cancel/…; prompt
handlers spawn a turn task and park a oneshot for the settle signal); (2) the
event pump (§3.3); (3) per-turn settle watchers (eval-rig `settle` logic:
first `StreamEnd` → then empty `Queue` or `System` error → resolve the
prompt's oneshot). A `session/cancel` notification routes to the turn task
via the session registry. All stdout writes go through one `Mutex<BufWriter>`
to preserve message framing; **nothing else may touch stdout** (logs →
stderr; the GUI binary branch exits before any Tauri init, so no stray
output).

### 3.7 Config & auth for ACP mode

- **Load the user's real config**, same as the app and the eval rig:
  `CoreBuilder::default_system()` reads `~/.config/tau/config.toml` (plus the
  project `.tau/config.toml` layer when a workspace opens — `config::load`).
  Config shape: `providers: {name: {base_url, key_env, models: {id:
  ModelDef}}}`, `generation.default_model`, etc. The API key itself is
  referenced by env var name (`key_env`), not stored.
- **Env-var overrides** (the Harbor channel — Harbor's launcher script
  `export`s the registry entry's `env` map into the agent's environment):
  `TAU_BASE_URL`, `TAU_API_KEY`, `TAU_MODEL` — when set, `tau acp` layers a
  synthetic provider entry on top of the loaded config (in-memory, same
  precedence as the project layer). This is the only way a Harbor container
  (fresh `HOME`, no config file) gets credentials. Names should be documented
  in the registry entry's `env` and in `tau acp --help`.
- **`authMethods` advertised (static, config-independent — §2.4 CI
  requirement):**
  1. `{ "id": "openai-compatible", "name": "OpenAI-compatible endpoint",
     "description": "Point tau at an OpenAI-compatible API (base URL + key)" }`
     — type defaults to `agent`; satisfies registry CI in any environment.
  2. (optional, only from the GUI binary) a `terminal` method that launches
     the tau TUI for interactive setup, the pi-acp `--terminal-login`
     pattern — the thin binary can't offer it (no TUI), so the registry entry
     ships method 1 only. Keep the code path, advertise conditionally.
- **`authenticate` behavior:** if a provider + key is already resolvable
  (config or env) → return `{}` (no-op success). Otherwise read
  `_meta` (documented extension, codex-acp precedent): e.g.
  `_meta: { "tau": { "baseUrl": …, "apiKey": …, "model": …? } }` → install
  the synthetic provider **for this connection only** (never write the
  user's config file as a side effect of `authenticate`). Unknown `methodId`
  → JSON-RPC `Invalid params`.
- **Model selection for Harbor:** `session/new` responds with
  `configOptions: [{id:"model", name:"Model", category:"model",
  type:"select", currentValue: <resolved default>, options: [all configured
  model ids]}]` and the server implements `session/set_config_option` →
  `Command::SessionSetModel`. Without this, Harbor's runner raises
  `"ACP agent did not advertise a model-selection mechanism"` whenever
  `HARBOR_ACP_REQUESTED_MODEL` is set (its `--model` flag).

### 3.8 What Harbor specifically expects (verified in harbor source)

`src/harbor/agents/installed/acp.py` + `acp_runner.py` (the generic
`AcpAgent`):

- Installs the agent from the **ACP registry** (`--agent acp:<id>`), a git
  source manifest, or a local command; writes a launcher shell script that
  `export`s the entry's `env` and execs `cmd args "$@"`; installs into
  `/opt/harbor-acp-agent`.
- Inside the environment it runs `acp_runner.py` with
  `--instruction <task>`; the runner (Python `acp` SDK,
  `spawn_agent_process`) drives: `initialize` (advertises
  `fs.read_text_file/write_text_file: true`, `terminal: true`,
  `auth.terminal: false`) → optional `authenticate` (policy `auto` **never**
  calls it for `agent`/`terminal` methods — only the deprecated `env_var`
  type; `explicit` calls the named id) → `new_session(cwd=workspace,
  mcp_servers)` → optional model set (session `models` legacy, else a
  `model`-category `config_options` select via `set_config_option`) →
  `prompt([text_block(instruction)])` → awaits the prompt response.
- `request_permission`: auto-allow (first `allow_once`/`allow_always`
  option) unless `HARBOR_ACP_PERMISSION_MODE=deny`.
- All `session_update`s are recorded verbatim to `acp-events.jsonl` and
  converted to the Harbor trajectory (tool calls from `tool_call`/
  `tool_call_update` with `rawInput`/`rawOutput`/`content`); the summary
  carries `prompt_response` (i.e. **`stopReason` is how Harbor sees
  completion**), `latest_usage_update`, `permissions_requested`.
- Env contract for a tau entry: the registry `env` map is the credential
  channel; `HARBOR_ACP_REQUESTED_MODEL` (if `--model` given) must be
  satisfiable via the model configOption.

**Full config surface Harbor injects** (verified in `acp.py` `run()` +
`acp_runner.py`): the launcher file (which `export`s the registry entry's
`env` map), plus seven runner-side vars the agent process *inherits* via
`child_env = dict(os.environ)`: `HARBOR_ACP_MCP_SERVERS_JSON`,
`HARBOR_ACP_PERMISSION_MODE`, `HARBOR_ACP_AUTH_POLICY`,
`HARBOR_ACP_AUTHENTICATE_METHOD_ID` (explicit policy only),
`HARBOR_ACP_REQUESTED_MODEL`, `HARBOR_ACP_AGENT_ID`,
`HARBOR_ACP_AGENT_VERSION`. None carry endpoint/key configuration — the
runner consumes them and translates them into protocol calls (requested
model → `set_config_option`; permission mode → auto-allow behavior). **The
agent must deliberately ignore all `HARBOR_ACP_*` vars**: the runner has
already applied them, and double-applying (e.g. the agent also enforcing a
permission mode) would be a bug.

**Consequence:** a Terminal-Bench-style task (prompt → tool calls → final
message → stop reason) works end-to-end with exactly the minimal v0 method
set + model configOption + env overrides — no `authenticate`, no
`requestPermission`, no `fs/*`.

---

## 4. What existing implementations do

### 4.1 pi-acp (svkozak/pi-acp, TypeScript, registry: `pi-acp` via npx)

A wrapper around the pi coding agent — the closest architectural analog to
tau (external CLI agent, no ACP natively).

- **Bridge:** per ACP session, **spawns a pi subprocess** and drives it over
  pi's own NDJSON RPC on the child's stdio (`src/pi-rpc/process.ts`); one
  ACP session = one pi process. pi events are translated to
  `conn.sessionUpdate(...)` via `@agentclientprotocol/sdk`
  (`AgentSideConnection` + `ndJsonStream` over the agent's own stdio).
- **Streaming:** pi's streamed message events become `agent_message_chunk`
  (text) and `agent_thought_chunk` (reasoning) directly — per-chunk, no
  snapshot diffing needed because pi emits deltas.
- **Tool mapping:** `translate/pi-tools.ts` maps pi tool events to
  `tool_call` (first sight) / `tool_call_update` (progress, result);
  `translate/bash.ts` defensively extracts the command/result text from many
  possible pi payload shapes.
- **Permissions:** pi's extension-UI confirm/choice events are forwarded to
  the ACP client via `conn.requestPermission` with yes/no options; on
  `session/cancel`, pending permission promises resolve `cancelled`.
- **Auth:** `authMethods` = one **terminal** method
  (`{id:"pi_terminal_login", type:"terminal", args:["--terminal-login"]}`);
  the entrypoint checks `process.argv` for `--terminal-login` and spawns the
  pi TUI interactively (`stdio: inherit`), exiting with its status. It also
  carries a Zed-specific `_meta["terminal-auth"]` launch spec — dual-shape for
  maximum client compat (a pattern to know about; tau doesn't need the Zed
  meta).
- **Mandatory vs optional (their call):** `loadSession: true` (pi sessions
  are files), `promptCapabilities.image: true`, unstable
  `sessionCapabilities.list/delete` for Zed's session picker; MCP servers
  "accepted and stored" (pi doesn't support them) — same accept-and-ignore
  stance recommended for tau.

### 4.2 claude-code-acp (Zed, TypeScript, registry: `claude-acp` npx)

- **Bridge:** **in-process** — wraps the `@anthropic-ai/claude-agent-sdk`
  `query()` object; no child process. The richest feature set of the three
  (modes, effort levels, subagents, compaction, file-change audit,
  elicitation, gateway auth).
- **Streaming:** SDK stream events → `session/update` via a
  `ToolCallReportingConnection` wrapper and per-tool-call reporters
  (`src/tool-calls/*`), with a field tracker to emit each changed field once.
- **Permissions:** full mapping of Claude Code permission modes
  (`acceptEdits`, `plan`, `bypassPermissions`, …) onto ACP
  `session/requestPermission` options (`src/permissions/*`); the SDK's
  permission callback is bridged to the client request.
- **Auth:** advertises `terminal` methods (launch `claude` CLI for login) +
  `gateway` methods (custom OpenAI-compatible/Bedrock gateways) — the
  gateway methods are the direct precedent for tau's
  OpenAI-compatible `authenticate`-via-`_meta` design. A
  `--hide-claude-auth` flag shows how auth advertisement is conditioned.
- **Takeaways:** capability-gated feature advertisement; `_meta` as the
  extension channel for gateway credentials; tool-call state tracked
  per-call with explicit field-diff reporting.

### 4.3 codex-acp (agentclientprotocol, TypeScript, registry: `codex-acp` npx)

- **Bridge:** **process wrapper** — drives the `codex app-server` subprocess
  over JSON-RPC (`CodexAppServerClient`/`CodexJsonRpcConnection`); ACP
  sessions map onto codex "threads".
- **Auth (the key reference):** four methods — `api-key` (**agent** type; the
  actual key arrives in the `authenticate` request's
  `_meta["api-key"].apiKey`; advertised based on env state), `chat-gpt`
  (agent, OAuth device flow the agent runs itself), `chat-gpt-device-code`,
  and `gateway` (custom model gateway via `_meta["gateway"]`). This is the
  canonical "agent type + `_meta` payload" pattern.
- **Streaming/events:** `CodexEventHandler` translates app-server item
  events (agent messages, reasoning, command executions, file changes, plan
  updates) into the matching `session/update` variants; `CodexPlanStream`
  for plans; `SteeringQueue` for mid-turn messages.
- **Takeaways:** multiple auth methods of mixed types on one agent; plans
  mapped from the native agent's plan object; cancellation threaded through
  to the subprocess.

### 4.4 sigit (getsigit/sigit, **Rust**, registry: `sigit` — binary + npx)

The only Rust-native ACP implementation in the registry worth studying;
local-inference coding agent, TUI + ACP mode.

- **CLI seam:** `--acp` flag parsed at the top of `main` (`parse_explicit_acp`
  rejects any additional arguments) → `run_acp_server` — the same top-of-main
  branch tau needs.
- **SDK use:** official `agent-client-protocol` crate (v1.3 + unstable
  features) with `Agent::builder().on_receive_request(...)` per method and
  `ByteStreams` over `tokio::io::stdin/stdout` (`.compat()` adapters).
- **Concurrency (the lesson to steal):** "Turn-affecting handlers below run
  in spawned tasks, serialized by `turn_lock`, so the dispatch loop stays
  free to route client responses (permission answers) while a turn is in
  flight. Awaiting a client request from *inside* a handler would deadlock:
  the dispatch loop can't read the response while the handler blocks it."
- **Auth:** local-model agent — its "auth" is model loading; honors
  `OPENAI_BASE_URL`/`OPENAI_API_KEY` env overrides in ACP mode (env-as-config
  precedent, same channel tau needs for Harbor).
- **Takeaways:** proof that the official Rust SDK works in a registry-listed
  agent; spawn+lock dispatch pattern; env overrides for provider config.

### 4.5 Registry (distribution) notes

- `agent.schema.json` required fields: `id, name, version, description,
  distribution` (+ `license_url` required per FORMAT.md; 16×16 `currentColor`
  SVG icon required per CONTRIBUTING.md; CI validates the schema, launches
  the agent in a sandbox, and runs the auth check of §2.4).
- `binary` distribution: per-platform
  `{archive (https, .zip/.tar.gz/…/raw), sha256?, cmd, args, env?}`;
  platforms `darwin-aarch64|x86_64`, `linux-aarch64|x86_64`,
  `windows-aarch64|x86_64`. Minimal tau entry:

```json
{
  "id": "tau", "name": "Tau", "version": "0.1.0",
  "description": "Tauri-based coding agent; ACP headless mode",
  "repository": "https://github.com/aaronlockhartdev/tau",
  "license": "AGPL-3.0", "license_url": "https://github.com/aaronlockhartdev/tau/blob/main/LICENSE",
  "icon": "…/tau/icon.svg",
  "distribution": { "binary": {
    "linux-x86_64":   { "archive": "https://github.com/…/releases/download/v0.1.0/tau-acp-linux-x86_64.tar.gz", "sha256": "…", "cmd": "./tau-acp" },
    "linux-aarch64":  { "…": "…" },
    "darwin-aarch64": { "…": "…" }
  } }
}
```

  (No `args` — the thin binary is ACP-only. `env` in the entry is where a
  Harbor deployment pins `TAU_BASE_URL`/`TAU_API_KEY`.)
- Process: fork, `mkdir tau/`, `agent.json` + `icon.svg`, PR; CI validates.
  A `preview` channel (separate version+distribution, `X.Y.Z-preview.N`)
  exists for unstable lines.

### 4.6 Patterns worth copying (ranked)

1. **Top-of-main CLI branch, ACP mode takes over before UI init** (sigit,
   pi-acp) — tau's seam is identical.
2. **Static `authMethods` + `agent` type with `_meta`-carried credentials**
   (codex-acp, claude-acp gateways) — satisfies registry CI in an empty env
   and is the in-band IDE path.
3. **Terminal-auth entrypoint flag** (`--terminal-login` spawns the
   interactive TUI, exit 0 = success) (pi-acp) — for the GUI binary's
   `authMethods`, later.
4. **Spawned turn handlers + per-session turn lock** (sigit) — avoids the
   dispatch-loop deadlock the moment an agent→client request exists.
5. **Accept-and-ignore MCP servers** (pi-acp) — honest v0 posture.
6. **Field-diff tool-call reporting** (claude-code-acp) — tau gets this for
   free from snapshot diffing.
7. **Env-var provider overrides** (sigit) — the Harbor channel.

---

## 5. Risks & open questions

1. **Bounded-pipe drops (residual).** (b) is effectively lossless at the
   16 ms cadence, but a client that stalls stdout for >~15 s (1024 events of
   headroom) triggers drops. Mitigation is designed (drop counter → journal
   resync), not yet measured. If real clients stall, the fix is the small
   core change (c). *Decision needed at review: accept (b) for v0.*
2. **Snapshot-diff chunking edge cases.** Reasoning and text grow in the
   same entry; interruption (`interrupted: true`) mid-snapshot; an entry
   re-emitted after compaction (`first_kept` entries) — the mapper must reset
   per-entry state on finalize, and compaction mid-turn needs a defined
   client-visible behavior (probably: nothing — compaction is invisible to
   ACP clients in v0).
3. **`authenticate` `_meta` extension is unofficial.** Precedent exists
   (codex-acp) and `_meta` is spec-sanctioned extension space, but there is
   no ACP-level standard for "agent reads credentials from `_meta`". IDE
   clients that don't know the extension can only use the terminal method or
   pre-configured env/config. Acceptable for v0 (Harbor + registry don't
   need in-band auth at all).
4. **Harbor env naming.** `TAU_BASE_URL`/`TAU_API_KEY`/`TAU_MODEL` are
   proposed, not settled. Must be stable once the registry entry ships
   (Harbor deployments pin them).
5. **Two binaries, one version.** The GUI binary (`tau acp`) and the thin
   `tau-acp` must not drift in protocol behavior — solved by sharing
   `crates/tau-acp`, but release process must build both from one tag.
6. **stdout hygiene in the GUI binary.** `tau acp` from the Tauri binary must
   guarantee no Tauri/wry code path writes stdout before the branch
   (trivially true if the branch is first in `main`), and the thin binary
   must route all `eprintln!`/tracing to stderr.
7. **`session/prompt` with non-text blocks.** v0 baseline requires accepting
   `text` + `resource_link`; the mapping of `resource_link` into tau's
   `MessageSend` (text-only) is undefined — v0: accept `text`, reject
   others with `Invalid params` (don't advertise image/audio/embeddedContext
   capabilities, which makes rejection spec-compliant).
8. **Concurrency beyond one prompt per session.** A second `session/prompt`
   while a turn is in flight lands in tau's FollowUp lane (queued) — the ACP
   server must decide: queue it (turn settles → second turn starts → respond)
   or reject with an error. The eval rig never exercises this; Harbor sends
   one prompt per session. *Recommendation: reject with a clear error in v0
   (one in-flight prompt per session) — simpler, and no client in the target
   set interleaves.*
9. **TCK gaps.** acp-tck is experimental; passing it is a goal, not a
   contract. Registry CI + Harbor are the real gates.
10. **v2.** When ACP v2 stabilizes, `authenticate`→`auth/login`/`logout` and
    the accept-then-idle prompt lifecycle change the server's turn state
    machine. Keeping the mapping layer isolated (§3.4) bounds that rework.

---

## 6. Sources

**ACP spec (primary).** <https://github.com/agentclientprotocol/agent-client-protocol>
- Transport/framing: `docs/protocol/v1/transports.mdx`
- Methods overview (baseline/optional, both sides): `docs/protocol/v1/overview.mdx`
- Initialization, versioning, capabilities: `docs/protocol/v1/initialization.mdx`
- Session new/load/resume/close, cwd, MCP servers: `docs/protocol/v1/session-setup.mdx`
- Prompt turn lifecycle, stop reasons, cancellation: `docs/protocol/v1/prompt-turn.mdx`
- `session/update` variant table: `docs/protocol/v1/prompt-turn.mdx` ("Session Updates")
- Tool calls, statuses, kinds, permission options: `docs/protocol/v1/tool-calls.mdx`
- Content blocks: `docs/protocol/v1/content.mdx`
- Auth methods, `authenticate`, `logout`: `docs/protocol/v1/authentication.mdx`
- Terminal-auth RFD (agent/terminal types, `_meta` history): `docs/rfds/auth-methods.mdx`
- Filesystem methods: `docs/protocol/v1/file-system.mdx`
- Config options (model selection): `docs/protocol/v1/session-config-options.mdx`
- Machine schema (all JSON shapes in §2 extracted from here): `schema/v1/schema.json`
  (defs: `InitializeRequest/Response`, `AgentCapabilities`, `AuthMethod*`,
  `NewSessionRequest/Response`, `PromptRequest/Response`, `StopReason`,
  `CancelNotification`, `SessionUpdate`, `ContentChunk`, `ToolCall*`,
  `RequestPermission*`, `PermissionOption*`, `ContentBlock`,
  `SetSessionConfigOptionRequest`)
- Versioning status: `CHANGELOG.md` (1.10.2, 2026-10-01); v2 = unstable
  (`docs/protocol/v2/`, `schema/v2/`, SDKs feature-gated)
- Rust SDK: <https://github.com/agentclientprotocol/rust-sdk> (crate
  `agent-client-protocol` 2.2.0; `docs/libraries/rust.mdx`)
- Python SDK (Harbor's client): <https://github.com/agentclientprotocol/python-sdk>
  (`src/acp/schema.py`)
- TCK: <https://github.com/agentclientprotocol/acp-tck> (`docs/libraries/testing.mdx`)

**Registry (primary).** <https://github.com/agentclientprotocol/registry>
- `FORMAT.md` (agent.json schema, binary/npx/uvx distributions, platforms,
  preview channel), `CONTRIBUTING.md` (PR process, icon rules),
  `AUTHENTICATION.md` (agent/terminal auth requirements)
- `agent.schema.json` (required fields)
- CI: `.github/workflows/verify_agents.py` (sandbox launch),
  `.github/workflows/client.py` (`run_auth_check`: isolated HOME, `initialize`,
  `validate_auth_methods` requires ≥1 `agent`/`terminal` method),
  `.github/workflows/protocol_matrix.py` (nightly v1 probes: initialize +
  session/new + unstable methods)
- Entries studied: `pi-acp/`, `claude-acp/`, `codex-acp/`, `sigit/`, `harn/`,
  `corust-agent/`, `opencode/`, `gemini/`

**Harbor (primary).** <https://github.com/harbor-framework/harbor>
- `src/harbor/agents/installed/acp.py` (registry/local install, launcher
  script with `env` exports, `run()` env contract: `HARBOR_ACP_MCP_SERVERS_JSON`,
  `HARBOR_ACP_PERMISSION_MODE`, `HARBOR_ACP_AUTH_POLICY`,
  `HARBOR_ACP_AUTHENTICATE_METHOD_ID`, `HARBOR_ACP_REQUESTED_MODEL`)
- `src/harbor/agents/installed/acp_runner.py` (the generic ACP driver:
  initialize→[authenticate]→new_session→[set model via `config_options`
  `model` category]→prompt; auto-allow `request_permission`; events →
  `acp-events.jsonl`; `stopReason` as completion signal)
- `src/harbor/agents/installed/acp_registry.py` (`acp:` shorthand)

**Implementations studied.**
- pi-acp: <https://github.com/svkozak/pi-acp> — `src/index.ts` (ndjson
  stdio, `--terminal-login` entry), `src/acp/agent.ts` (initialize/caps),
  `src/acp/auth.ts` (terminal auth method), `src/acp/session.ts` (streaming,
  permission forwarding, cancel), `src/pi-rpc/process.ts` (pi subprocess
  RPC), `src/acp/translate/*`
- claude-code-acp: <https://github.com/zed-industries/claude-code-acp> —
  `src/acp-agent.ts` (initialize, authMethods incl. gateway),
  `src/hide-claude-auth.ts`, `src/permissions/*`, `src/tool-calls/*`
- codex-acp: <https://github.com/agentclientprotocol/codex-acp> —
  `src/CodexAuthMethod.ts` (api-key via `_meta`, gateway),
  `src/CodexAppServerClient.ts` (app-server subprocess),
  `src/CodexEventHandler.ts`
- sigit: <https://github.com/getsigit/sigit> — `Cargo.toml`
  (`agent-client-protocol 1.3`), `src/main.rs` (`parse_explicit_acp`,
  `run_acp_server`, spawn+turn_lock dispatch comment)

**tau (internal, /Users/aaron/git/tau).**
- `app/src-tauri/src/main.rs` (flat `main()`, the seam)
- `crates/tau-eval/src/live.rs` (production-shape core, `core.events()`
  fan-out pump, dispatch drive, settle rule, journal tailing)
- `crates/tau-core/src/harness/state.rs` (`Core`/`CoreBuilder`,
  `events_tx` 1024 bounded, `pipe_send` drop counting, `Core::events()`
  single-tap)
- `crates/tau-core/src/harness/pump.rs` (the GUI pump)
- `crates/tau-core/src/harness/forwarding.rs` + `src/agent/turn.rs`
  (`StreamEntrySink`, `SNAPSHOT_INTERVAL = 16 ms`, wire-only upserts,
  call/entry id pairing)
- `crates/tau-core/src/harness/launch.rs` (`wire_events` hooks,
  `SessionRole::Root`, `root_session`)
- `crates/tau-core/src/session/file.rs` (append-only journal, torn-tail
  settling, one line per entry)
- `crates/tau-core/src/config.rs` (`Provider {base_url, key_env, models}`,
  `Generation.default_model`, `config::load` system+project layering)
- `crates/tau-protocol/src/lib.rs` (`Command`/`CommandOutput`:
  `WorkspaceOpen`, `SessionNew`, `SessionSetModel`, `MessageSend`,
  `MessageStop`), `src/events.rs` (`Event` enum), `src/payload.rs`
  (`AssistantPayload`, `ToolPayload`), `src/snapshot.rs` (`ViewEntry`,
  `SessionMeta`)
- `crates/tau-core/src/tools.rs` (tool names: read, write, edit, bash,
  recall, subagent_*, task_*)
- `docs/adr/0007-v0-security-transparency-not-enforcement.md` (no
  permissions in v0), `docs/adr/0008-live-transcript-stream-file-lines-upsert-by-id.md`
  (16 ms snapshot streaming model, last-per-id resync)
