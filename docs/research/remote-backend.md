# Research: Can tau's GUI connect to a remote back-end?

Issue: [#13](https://github.com/aaronlockhartdev/tau/issues/13) · Branch: `research/remote-backend` · Researched 2026-09-16

**Question.** Can tau's GUI (Svelte/Tauri, or a browser front-end) drive `tau-core` running on another machine? v0 stays a single local process per [ADR-0002](../adr/0002-single-tauri-process-with-standalone-core.md); this research decides what v0 must keep true so the transport can later be swapped without re-architecture.

**Short answer.** Yes — and every prior art examined does exactly this with a *versioned, serializable, core-owned command/event message set* where the front-end is a stateless renderer over a transport. If tau's v0 core↔GUI protocol (ticket #10) is defined as such a message set — with the session state, file paths, and streaming reassembly owned by the core, never the GUI — then "remote" becomes a new transport (WebSocket/HTTP client binary wrapping `tau-core`) plus auth, not a re-architecture. The constraints are cheap in v0; the machinery (auth, multi-client, session sync) is legitimately deferred.

---

## 1. v0 protocol constraints for transport-agnosticism

What the core↔GUI protocol of ticket #10 must guarantee so the Tauri IPC transport can be replaced by a network transport later:

### 1.1 A versioned, serializable message set

- **All traffic must be instances of one versioned, JSON-serializable tagged-union type** (commands in, events out, plus request/response sub-messages for HITL). Tauri commands/events are then just transport #1 carrying those same types; a future `tau serve` binary and a browser client are transport #2/#3. This is exactly pi's RPC mode: "**Commands**: JSON objects sent to stdin, one per line"; "**Responses**: JSON objects with `type: \"response\"`"; "**Events**: Agent events streamed to stdout as JSON lines", with an optional `id` on commands for request/response correlation (pi `docs/rpc.md`, "Protocol Overview"; the command type list in [pi-mono `packages/coding-agent/src/modes/rpc/rpc-types.ts`](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/src/modes/rpc/rpc-types.ts) — plain `type`-tagged structs, all `JSON`-representable).
- **The protocol version must be declared on the wire at connect** and mismatches must be detectable. pi RPC has *no* protocol version field — its only versioning is in the session file header (`"version": 3`, auto-migrated v1→v2→v3, pi `docs/session-format.md`, "Session Version"). OpenCode is the better model: its server exposes `GET /global/health → { healthy, version }` and an OpenAPI 3.1 spec at `/doc` ("use the spec to generate clients or inspect request and response types", opencode `docs/server`). Claude Code documents version-gated flags (e.g. `--continue` on `claude remote-control` "requires Claude Code v2.1.200 or later; earlier versions reject the flag", code.claude.com `docs/en/remote-control.md`). *Tau should adopt the version-at-handshake rule in v0 — it is nearly free to add now and expensive to retrofit.*
- **Changes to the set must be additive**: new message variants are additions; fields are optional. The pi event table (≈25 event types, pi `docs/rpc.md` "Event Types") shows the realistic surface; tau's event set from ticket #10 (turn progress, tool-call start/update/end with output, sub-agent lifecycle, compaction, queue state) maps onto it 1:1.

### 1.2 No local-file assumptions in the data model

- **Message payloads must never embed a host-local path as identity.** In pi, the session header stores `cwd: "/path/to/project"` and session files live at `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl` keyed by the *local* working directory (pi `docs/session-format.md`, "File Location"). That is a local-file assumption baked into the data model; a remote GUI has no such path. Tau's v0 data model should instead carry a **workspace** object (id, display name, remote cwd as an attribute owned by the core) and express file locations as paths *in that workspace* — code-server is the prior art for path semantics that work remotely: its URL API takes `?folder=/home/coder/project` (absolute, remote) and `vscode-remote://<host>/<absolute path>` URIs, with the explicit rule "paths must be absolute; there is no form relative to `folder`" (code-server `docs/FAQ.md`, "How does code-server decide what workspace or folder to open?").
- **Tool args are workspace-relative data, not handles to the client's filesystem.** When tools run remotely they act on the remote filesystem — the JetBrains remote-development model is "a project located in the remote file system" managed by the remote IDE backend (JetBrains Toolbox App help, "Manage remote projects"; jetbrains.com/remote-development: "Remote machine doing all the heavy lifting… Your code and data stay on the server").

### 1.3 Session-state ownership: core-only, GUI-reconstructible

- **The core is the single owner of session state; the GUI must be rebuildable from (snapshot + incremental events), never the other way around.** pi's session is "an append-only tree of entries with stable ids, so an entry id works as a durable cursor: pass the last entry id you have seen as `since` to get only entries strictly after it, *even across client restarts*" (pi `docs/rpc.md`, `get_entries`; tree structure in `docs/session-format.md`). The `leafId` returned with entries "let[s] a client tell in one round trip whether the active branch moved". Tau's sessions are the same shape (a tree of entries, per `CONTEXT.md`) — so the v0 rule: **every GUI state must be derivable from `get_entries(since) + leafId`-style core queries**; the GUI keeps no authoritative copy. This is what makes a second, remote GUI possible: it is just another stateless renderer.
- pi's RPC `get_messages`/`get_state` return full snapshots, and the TUI/RPC client is exactly this pattern (pi `docs/rpc.md` "State"; `src/modes/rpc/rpc-client.ts` referenced there). OpenCode's server API does the same: `GET /session/:id` details, `GET /session/:id/message` (list), `DELETE /session/:id` "delete a session and all its data" — session data is server-owned (opencode `docs/server`).
- **HITL (permission prompts, confirmations) must be a request/response message pair with a timeout**, not a UI channel. pi's RPC "Extension UI Protocol" is the direct prior art: `extension_ui_request` (dialog methods `select/confirm/input/editor`) is emitted on stdout and *blocks* until the matching `extension_ui_response` arrives on stdin; "if a dialog method includes a `timeout` field, the agent-side will auto-resolve with a default value" (pi `docs/rpc.md`). Claude Remote Control runs the same idea over the network: "while the connection is rebuilding, Claude Code queues messages, permission prompts, and status updates from subagents and workflows, and delivers them once the connection recovers" (code.claude.com `docs/en/remote-control.md`).

### 1.4 Streaming semantics that survive a network

- **Stream deltas must be self-contained, ordered, and reassemblable by the client.** pi's `message_update` is "a delta event *without* a cumulative message snapshot"; clients "must assemble it from `message_start` and subsequent events using `contentIndex`" and "treat `message_end.message` as authoritative" (pi `docs/rpc.md`). Tool output streams as `tool_execution_update` whose `partialResult` is "the accumulated output so far (not just the delta), allowing clients to simply replace their display on each update" — i.e. **idempotent replace-on-receive**, which is exactly what a lossy/reordering-possible network wants. Correlation by `toolCallId`/command `id` lets any transport multiplex (pi `docs/rpc.md`).
- **Images/attachments travel as base64 in-message payloads**, not file references — pi's `prompt`/`steer` take `images: [{type:"image", data:"base64…", mimeType}]` (pi `docs/rpc.md`), and Claude Remote Control does the same across its relay: "send images and files from your phone or browser… Claude Code downloads other files to your machine and passes them to Claude as `@` file references" (code.claude.com `docs/en/remote-control.md`).
- **No cross-process shared memory or file watching between core and GUI.** Tauri v0 is already this way (command/event boundary, ADR-0002), and it is precisely what keeps a network transport honest: everything crosses the one boundary as messages.

---

## 2. Prior art (with citations)

All URLs retrieved 2026-09-16.

### pi (this project's design donor)

- **RPC mode** — headless JSON protocol over stdin/stdout; full command/event tables, framing rules ("strict JSONL semantics with LF as the only record delimiter"), extension UI request/response sub-protocol. Source: local install `docs/rpc.md` of `@earendil-works/pi-coding-agent` (canonical source: [pi-mono `packages/coding-agent/src/modes/rpc/`](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/src/modes/rpc/rpc-types.ts)). *This is the in-local-process sibling of the remote question: same protocol shape, stdio transport.*
- **Session format** — JSONL entry tree, header `version` 1/2/3 with auto-migration, files keyed by cwd. Source: pi `docs/session-format.md` (source: pi-mono `packages/coding-agent/src/core/session-manager.ts`).
- **Containerization** — "run the whole `pi` process inside an isolated environment" (Docker, OpenShell "local or remote managed sandbox") as a way to run the harness on another machine/VM while keeping the same client story. Source: pi `docs/containerization.md`.
- **SDK vs RPC separation** — Node users are pointed at the in-process `AgentSession` API; subprocess clients use the RPC protocol (pi `docs/rpc.md`, note at top). ADR-0002 deliberately mirrors this ("so future binaries (CLI, RPC) can reuse it").

### JetBrains Gateway / remote development

- **Architecture** — the full IDE *backend* runs on the remote machine; the local client (Gateway, now folded into the Toolbox App) is "a lightweight IDE interface that feels local" over a "secure SSH connection" to a "remote dev environment" running "IDE's backend / tools and runtimes" (jetbrains.com/remote-development). Explicitly **not** a pixel stream: "no more input lag and pixelated video streams — just smooth, real-time coding"; "your code and data stay on the server".
- **Auth is the transport's, not the app's** — Toolbox App "leverages OpenSSH… supports advanced features such as ProxyJump, MFA, reverse proxy, custom IdentityFile settings, askpass integration, and even the ability to replace the SSH binary" (jetbrains.com/remote-development, FAQ); the connection is just `h`ostname/`u`sername/`p`ort (jetbrains.com/help/toolbox-app/gettings-started-with-ssh.html, `jetbrains://gateway/ssh/environment?…` universal link).
- **Remote-owned workspace + auto-reconnect** — "connect to a remote server using the SSH connection to open or manage a project located in the remote file system"; "if you encounter a problem connecting to your server, the attempt to reconnect will be established automatically by default" (same Toolbox App help page).

### code-server (VS Code in the browser)

- **Architecture** — "run VS Code on any machine anywhere and access it in the browser"; "all intensive tasks run on your server"; requires "WebSockets… code-server uses WebSockets to communicate between the browser and the server" (code-server `README.md`, `docs/requirements.md`; github.com/coder/code-server).
- **Auth** — password auth by default with rate limits ("two per minute plus an additional twelve per hour"); "never expose code-server directly to the internet without some form of authentication and encryption, otherwise someone can take over your machine via the terminal"; blessed exposure patterns: SSH port-forwarding with `auth: none` behind the tunnel, or TLS reverse proxy (Let's Encrypt + Caddy/NGINX) (code-server `docs/guide.md`).
- **Remote workspace semantics** — workspace chosen by `?workspace`/`?folder` query param (absolute remote path) or command line; deep links use `vscode-remote://<host>/<absolute path>` (code-server `docs/FAQ.md`).
- **Collaboration is out-of-band** — code-server itself is one-user; real-time sharing is only via third-party extensions (Duckly, CodeTogether) (code-server `docs/collaboration.md`).

### OpenCode (agent harness with a built-in server)

- **Architecture** — "`opencode serve` runs a headless HTTP server that exposes an OpenAPI endpoint that an opencode client can use"; `opencode` (TUI) "starts a TUI and a server. The TUI is the client that talks to the server"; "this architecture lets opencode support multiple clients and allows you to interact with opencode programmatically" (opencode.ai/docs/server; page content as archived 2026-01-15).
- **Auth** — HTTP basic auth via `OPENCODE_SERVER_PASSWORD` (optional `OPENCODE_SERVER_USERNAME`); binds `127.0.0.1:4096` by default, with `--hostname`/`--cors` to expose (same page).
- **Wire shape** — REST + SSE: `GET /global/event` SSE stream ("first event is `server.connected`, then bus events"), `/event` per-scope, session CRUD + `POST /session/:id/message`, `prompt_async`, `/session/:id/fork`, `abort`, `share`, `permissions/:permissionID` (client responds to permission requests), plus file tools (`/file`, `/find`) that operate on the server's filesystem (same page).
- **Version/compat surface** — `/global/health → { healthy, version }`, OpenAPI spec at `/doc` (same page).

### Claude Code / Claude Agent SDK (Anthropic)

- **Remote Control — local backend, many remote front-ends** — `claude remote-control` "starts a server" on your machine; claude.ai/code, the mobile app, and the terminal "stay in sync across all connected devices, so you can send messages from your terminal, browser, and phone interchangeably"; "Claude keeps running locally the entire time, so your code execution and filesystem access stay on your machine"; "the web and mobile interfaces are a window into that local session" (code.claude.com/docs/en/remote-control.md). Multi-client with capacity: `--capacity <N>` "maximum number of concurrent sessions. Default is 32"; `--spawn same-dir|worktree|session` gives per-session isolation (git worktree per session) or strict single-client mode; `--permission-mode` and `--sandbox` set remote-session policy (same page).
- **Cloud sessions — remote backend, many front-ends** — "a cloud session is a Claude Code session that runs on cloud infrastructure instead of on your machine… the session keeps running after you close your laptop, and you can check on it or steer it from any device" (browser, mobile, desktop, terminal) (code.claude.com/docs/en/claude-code-on-the-web.md). Workspace provisioning is remote-owned: "the cloud VM clones your current directory's GitHub remote at your current branch"; GitHub access is authorized via the Claude GitHub App or `/web-setup` (sends your `gh` token); `--teleport` pulls a cloud session back into a terminal (same page).
- **Headless/SDK** — `claude -p` runs the same harness non-interactively; the Claude Agent SDK (TypeScript/Python) is "built on top of the agent harness that powers Claude Code" and is the sanctioned way to embed the agent loop in a service (code.claude.com/docs/en/headless; docs.anthropic.com/en/api/agent-sdk/overview, as archived 2025).

---

## 3. What changes on the wire (local Tauri → remote)

| Concern | Local (v0, today) | Remote (later) | Prior-art precedent |
|---|---|---|---|
| **Auth/credentials** | None needed — Tauri IPC is OS-scoped to the app | Token/TLS on the channel; *provider* credentials (v0: OpenAI-compatible base URL + key, `CONTEXT.md`) must be configured **on the server** (whoever runs `tau-core`), never shipped from the GUI; GUI-side auth is identity only | code-server: password/basic-auth on the server, `auth: none` behind SSH (guide.md); OpenCode: `OPENCODE_SERVER_PASSWORD` basic auth (docs/server); JetBrains: OpenSSH (keys, MFA, ProxyJump) — auth belongs to the transport (remote-development, Toolbox help); Claude: claude.ai sign-in + relay (remote-control.md), GitHub App/`gh`-token for repo access (claude-code-on-the-web.md) |
| **Remote-workspace semantics** | cwd is a local path the app owns | The core owns a **workspace** (id, name, cwd-on-host); tool paths resolve in the *server's* filesystem; the GUI only ever sees workspace-relative/remote-absolute paths; file reads for preview/diff are fetched as content payloads | code-server `?folder=` absolute + `vscode-remote://<host>/<abs path>` (FAQ); JetBrains "project located in the remote file system" (Toolbox help); OpenCode `/project`, `/file`, `/find` server APIs (docs/server) |
| **Where session files live & sync** | Local disk (pi-style per-cwd store) | **Remote-owned**: the server stores sessions; the local GUI is a mirror/cache. Sync = the snapshot/cursor API (`entries since <id>` + `leafId`), not file replication; optional export/share is an explicit message | pi: per-cwd local JSONL, `get_entries(since)` cursor (session-format.md, rpc.md); OpenCode server-side session store + `DELETE /session/:id` + `/share` (docs/server); Claude cloud sessions persist server-side, `--teleport` moves a session between surfaces (claude-code-on-the-web.md) |
| **Multi-client on one back-end** | One GUI, one core (v0) | Core must serve N renderers of the same session (desktop + browser/phone) and/or N sessions; needs per-client cursors, presence, and a policy for concurrent writers (e.g. worktree-per-session) or a single-writer rule | Claude Remote Control: all devices "stay in sync… interchangeably", `--capacity 32` default, `--spawn worktree` isolation, `--spawn session` strict single-client (remote-control.md); OpenCode: "support multiple clients" (docs/server); code-server: deliberately *not* multi-user (collaboration.md) |
| **Latency/UX** | Zero-latency IPC | Streaming stays fine (deltas are small), but: permission round-trips need timeouts + queuing; reconnect must re-sync via cursor; optimistic/queued UX for drops | Claude RC: "queues messages, permission prompts, and status updates… delivers them once the connection recovers" + auto-reconnect (remote-control.md); JetBrains: "indistinguishable from local… no pixelated video streams" — the data-over-the-wire design (remote-development); pi: idempotent cumulative `partialResult`, `contentIndex` reassembly (rpc.md) |
| **Version compatibility** | One binary, one version | Client and server binaries may drift: handshake must exchange protocol versions and reject/limit gracefully; the *session file* format is a separate, independently versioned surface (pi: header `version` 1/2/3, auto-migrated) | OpenCode: `/global/health {version}` + OpenAPI `/doc` (docs/server); pi: session versioning (session-format.md); Claude: version-gated CLI flags (remote-control.md) |

---

## 4. Recommendation: adopt now (cheap) vs defer (legitimate)

### Adopt in v0 now (each is cheap at this stage, expensive to retrofit)

1. **Define the core↔GUI protocol as a versioned tagged-union message crate.** One shared Rust crate owns `ProtocolVersion`, the command union, the event union, and the HITL request/response pair. Every Tauri command and event is an instance of it (ticket #10 should be written against this crate, not ad-hoc Tauri signatures). Cost: a few days of type definition. Pays off: the `tau serve` binary is a transport shim, not a protocol redesign. *(pi's rpc-types.ts is the shape; OpenCode's `/global/health` + spec is the compat surface.)*
2. **Core owns all session state; GUI is rebuildable from snapshot + cursor.** Guarantee the `get_entries(since=<stable entry id>) → {entries, leafId}`-style query exists in the v0 API, and that the Svelte store can be reconstructed from it plus the live event stream. *(pi rpc.md `get_entries`/`get_tree`.)*
3. **No local paths in the data model.** Introduce the workspace object (id, name, cwd) in v0's session header (mirroring pi's header minus the local-path identity); tool args carry paths relative to that workspace; previews/diffs are content payloads, never client paths. *(code-server FAQ path rules; pi session header.)*
4. **Streaming = start/delta/end + idempotent cumulative updates, correlated by stable ids** (message id, `toolCallId`, sub-agent session id). This is what the GUI needs locally anyway; it is exactly what a network needs. *(pi `message_update`/`tool_execution_update`.)*
5. **HITL = request/response messages with timeouts + queueing on disconnect.** The permission UI is a message pair in v0 (it must be, for the Tauri event boundary); give it a timeout/default resolution and make the core queue while a GUI is absent. *(pi extension-UI protocol; Claude RC queuing.)*
6. **Keep ADR-0002's library boundary honest:** `tau-core` stays free of Tauri types; the v0 Tauri adapter is the *only* in-process transport implementation. (ADR-0002 already mandates this — it just needs to be enforced in review, since it is the whole bet.)

### Legitimately deferred (do not pay for in v0)

- **Auth/TLS and server hardening** — token scheme, TLS, binding policy, rate limiting. v0 is a local OS-scoped IPC; adopt when the server binary ships. *(code-server/OpenCode patterns ready to copy.)*
- **The `tau serve` network binary and browser client** — pure transport addition once §4.1–4.6 hold. *(pi: "future binaries (CLI, RPC) can reuse it" — ADR-0002.)*
- **Multi-client policy** — concurrent-writer rules, per-client cursors/presence, capacity, worktree-per-session isolation. Design note only for v0: keep the *single active writer per session* rule trivially true by having one core; add `--spawn`/`--capacity`-style options with the server. *(Claude RC flags.)*
- **Session sync/replication and share** — remote-owned store with local mirror is already implied by §4.2; explicit export/share/teleport features come with the server. *(OpenCode `/session/:id/share`, Claude `--teleport`.)*
- **Latency-specific UX** — reconnect choreography, optimistic UI, bandwidth heuristics; build against the queued/HITL primitives once the transport exists. *(Claude RC reconnect/queue.)*
- **A wire-protocol migration engine** — a version field + additive rule is enough until two versions exist in the wild; the session-format migration story (pi-style header version) can be written when the first breaking change lands.

**Bottom line for the map:** the remote GUI is a *transport addition* — a `tau serve` binary wrapping the same `tau-core` over WebSocket/HTTP, with a token — if and only if v0 keeps the six constraints in §4. Nothing in the prior art (pi RPC, JetBrains, code-server, OpenCode, Claude Remote Control/cloud) requires the front-end to own state, embed local paths in messages, or use a transport-specific message shape. v0 should cost ~zero for that; every prior art paid for it later at higher price.

---

## Sources

Retrieved 2026-09-16.

- pi (local install of `@earendil-works/pi-coding-agent`): `docs/rpc.md`, `docs/session-format.md`, `docs/containerization.md` — canonical sources in [earendil-works/pi-mono](https://github.com/earendil-works/pi-mono) (`packages/coding-agent/src/modes/rpc/rpc-types.ts`, `src/core/session-manager.ts`).
- JetBrains: [Remote Development overview](https://www.jetbrains.com/remote-development/); [JetBrains Gateway](https://www.jetbrains.com/remote-development/gateway/); [Toolbox App — Manage remote projects / SSH](https://www.jetbrains.com/help/toolbox-app/gettings-started-with-ssh.html).
- code-server (github.com/coder/code-server, raw docs): [`README.md`](https://raw.githubusercontent.com/coder/code-server/main/README.md), [`docs/guide.md`](https://raw.githubusercontent.com/coder/code-server/main/docs/guide.md), [`docs/requirements.md`](https://raw.githubusercontent.com/coder/code-server/main/docs/requirements.md), [`docs/FAQ.md`](https://raw.githubusercontent.com/coder/code-server/main/docs/FAQ.md), [`docs/collaboration.md`](https://raw.githubusercontent.com/coder/code-server/main/docs/collaboration.md).
- OpenCode: [opencode.ai/docs/server](https://opencode.ai/docs/server/) (content via Wayback Machine snapshot; page dated Jan 15, 2026).
- Claude Code / Agent SDK: [code.claude.com/docs/en/remote-control.md](https://code.claude.com/docs/en/remote-control.md), [code.claude.com/docs/en/claude-code-on-the-web.md](https://code.claude.com/docs/en/claude-code-on-the-web.md), [code.claude.com/docs/en/headless](https://code.claude.com/docs/en/headless), [docs.anthropic.com/en/api/agent-sdk/overview](https://docs.anthropic.com/en/api/agent-sdk/overview) (via Wayback, 2025).
- tau: [ADR-0002](../adr/0002-single-tauri-process-with-standalone-core.md), `CONTEXT.md`, [issue #10](https://github.com/aaronlockhartdev/tau/issues/10).
