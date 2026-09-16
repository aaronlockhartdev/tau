# How Do Existing Harnesses Model Sub-Agents and Tasks?

Research for wayfinder ticket #5 (feeds the decision ticket #6, "What is tau's sub-agent & task model?").
Date: 2026-09-16. Method: primary sources only — installed package source, official documentation sites, and the mastra repo at a pinned commit.

**Sources**

| System | Primary source |
|---|---|
| pi-cohort 6.1.0 | Installed package at `/Users/aaron/.pi/agent/npm/node_modules/pi-cohort` — `README.md`, `skills/pi-cohort/SKILL.md`, `src/extension/schemas.ts`, `src/shared/fork-context.ts`, `src/shared/artifacts.ts`, `src/runs/background/async-resume.ts`, `src/runs/shared/subagent-prompt-runtime.ts`, `src/runs/shared/workflow-graph.ts`, `src/runs/shared/pi-spawn.ts`, `agents/worker.md` (repo: github.com/jjuraszek/pi-cohort) |
| Claude Code | Official docs: [Create custom subagents](https://code.claude.com/docs/en/sub-agents) and [Agent SDK subagents](https://code.claude.com/docs/en/agent-sdk/subagents) (accessed 2026-09-15) |
| Mastra | github.com/mastra-ai/mastra @ commit `317faeb2` (2026-09-15, main): `docs/src/content/en/docs/subagents.mdx`, `packages/core/src/agent/agent.ts`, `packages/core/src/agent/subagent.ts`, `docs/src/content/en/docs/workflows/{overview,snapshots,suspend-and-resume}.mdx` |
| OpenCode | Official docs: [Agents](https://opencode.ai/docs/agents/) (accessed 2026-09-16) |

For each system: session topology, hand-off protocol, concurrency & observation, lifecycle.

---

## 1. pi-cohort (pi coding agent extension)

pi-cohort adds a single `subagent()` tool to the pi parent session. One parent stays in control; children are focused single-job sessions ("a team with one orchestrator, not a swarm").

### Session topology — separate linked session files, separate processes

- A subagent is "a focused child Pi session with its own job" — a separate process, not an in-process sub-conversation (README, "Mental model").
- `context: "fresh"` (default) starts a clean session; `context: "fork"` creates **a real branched session file**: the extension calls `SessionManager.open(parentSessionFile).createBranchedSession(leafId)` on the persisted parent session and caches one branch file per child index (`src/shared/fork-context.ts`). Forking "requires a persisted parent session" and fails otherwise (SKILL.md, "Important Constraints"). So the on-disk topology is *separate linked files*: the child's `.jsonl` session lives next to the parent's, linked by the branch and by run metadata.
- Nesting is a depth cap, not unlimited recursion: "Default subagent nesting depth is 2" (`maxSubagentDepth`), and a child can fan out only if its agent definition's `tools` include `subagent` ("Recursion guard", README; "Important Constraints", SKILL.md).

### Hand-off protocol — compact contract prompt + artifacts, never raw history

- The unit of hand-off is a **task prompt ("compact contract", not a procedural script)**: goal, context/evidence (file paths, plan paths, decisions already approved), success criteria, hard constraints, validation, output shape, stop rules (SKILL.md, "Prompting role subagents").
- What the child receives depends on flags: agent `systemPrompt`, optional `defaultReads` (e.g. `worker` reads `context.md, plan.md` — `agents/worker.md` frontmatter), optionally inherited project context/skills (`inheritProjectContext`, `inheritSkills`; `src/runs/shared/subagent-prompt-runtime.ts` strips the parent's project-context/skills/orchestration sections from inherited prompts when disabled).
- Crucially, the child **does not** receive the parent's conversation history by default: "Spawned subagents do not receive the `pi-cohort` skill, parent-only status/control/slash messages, or prior parent `subagent` tool-call/tool-result artifacts" (SKILL.md, "Keep orchestration authority in the parent session"), and parent-only custom message types are filtered out of inherited history (`subagent-prompt-runtime.ts`, `PARENT_ONLY_CUSTOM_MESSAGE_TYPES`).
- Chain hand-off is templated: step N+1's task can reference `{previous}` (the previous step's output) or named outputs `{outputs.name}` bound via `as:` (`src/extension/schemas.ts`, `ChainItem`). With `outputMode: "file-only"`, `{previous}` is a compact reference — "Output saved to: /abs/report.md (48.2 KB, 2847 lines). Read this file if needed." — instead of the full content (SKILL.md, "Parallel execution").
- On-disk artifacts per run: `<session-dir>/subagent-artifacts/{runId}_{agent}[_{index}]_{input.md, output.md, .jsonl, _meta.json}` (`src/shared/artifacts.ts`) — the hand-off is materialized as files the GUI can index.
- Structured results: `outputSchema` makes the child call a `structured_output` tool with schema-valid JSON or the step fails (`src/extension/schemas.ts`, `structured-output.ts` instructions in `subagent-prompt-runtime.ts`).

### Concurrency & observation — one tool, four dispatch shapes; parent watches via status tree, events, and delivery

- Dispatch shapes: **single**, **parallel** (`tasks[]`, `concurrency` default 4), **chain** (sequential steps, nested parallel groups, and dynamic fan-out via `expand`/`collect` from a producer's structured output), and **async** (any of the above backgrounded) (README "Architecture"; `SubagentParams` in `src/extension/schemas.ts`).
- Foreground runs stream into the parent conversation; async runs persist state in an async directory (`status.json`, `events.jsonl`, result JSON) and the pi runtime **delivers the completion** to the parent — "Pi will deliver the async completion when it arrives" (SKILL.md, "Async/background"), so the parent need not poll.
- `subagent({ action: "status", id })` returns active runs as a **tree**, including nested runs targetable by nested id (SKILL.md); chains additionally build a `WorkflowGraphSnapshot` — nodes for sequential steps, parallel groups, and dynamic children, each with status, phase, label, `acceptanceStatus`, error — the data a GUI would render (`src/runs/shared/workflow-graph.ts`).
- Control layer: no-activity past a threshold emits a `needs_attention` control event (persisted to `events.jsonl` for async, pushable for foreground); a **soft `interrupt` cancels the current child turn and leaves the run `paused`** — explicitly not success/failure (SKILL.md, "Subagent control"). Optional `monitor` child watches a run directory on an interval (SKILL.md, "Long-running job hygiene"). Session-wide cost of main loop + every child rolls up into one `Σ$` total (README, "Key concepts").

### Lifecycle — spawn / complete / kill(soft) / resume-as-revive

- Spawn = launch a child `pi` process (native execution backend; `src/runs/shared/pi-spawn.ts`; an external `execution-backend` API allows other execution surfaces).
- States: `queued`, `running`, `paused`, `complete`, `failed` (SKILL.md, "Subagent control").
- **Kill** = soft interrupt → paused, recoverable.
- **Resume = "revive"**: a finished/failed/paused child is revived by *starting a new child process from its persisted child session file* with a generated preamble — "You are reviving a previous subagent conversation… Use the stored session context as background" (`src/runs/background/async-resume.ts`, `buildRevivedAsyncTask`). A running child cannot be resumed; a child with no persisted `.jsonl` cannot be resumed (SKILL.md, "Resume behavior"). Multi-child runs need an `index` to pick which child revives.
- **Acceptance contract**: a per-run `acceptance` field with levels `auto/none/attested/checked/verified/reviewed`; `verified` requires the runtime to run explicit validation commands; `reviewed` requires an independent reviewer result ("Do not call a run reviewed just because the worker says it is done", SKILL.md); evidence kinds include `changed-files`, `tests-added`, `commands-run`, `validation-output`, `residual-risks`, `no-staged-files`, `diff-summary`, `review-findings`, `manual-notes` (`src/extension/schemas.ts`, `AcceptanceEvidenceKind`). Children hitting an unapproved decision stop with `BLOCKED: <decision needed>` / `Done:` / `Remaining:` (SKILL.md, "Stop on unapproved decisions").
- Write-safety: `worktree: true` gives each parallel writer its own git worktree branched from HEAD; the recommended default is a **single writer thread** with read-only advisors around it (SKILL.md, "Worktree Isolation", "Keep writes single-threaded by default").

---

## 2. Claude Code (Task/Agent tool + `.claude/agents` definitions)

### Session topology — in-sub-session contexts with separately persisted transcripts

- "Subagents work within a single session" and "Each subagent starts with a fresh, isolated context window. It doesn't see your conversation history" (sub-agents docs, intro + "What loads at startup").
- Transcripts persist **as separate files linked under the parent session**: `~/.claude/projects/{project}/{sessionId}/subagents/agent-{agentId}.jsonl`; they "persist independently of the main conversation" (survive main-conversation compaction), live for the session's lifetime, and are swept after `cleanupPeriodDays` (30 days default) (sub-agents docs, "Resume subagents").
- Nesting is a tree: subagents can spawn subagents up to `CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH` (default 3 layers; v2.1.219+); at the depth limit the `Agent` tool is withheld (a fork at the limit gets an error instead); the UI shows nested subagents "as a tree in the subagent panel … marks each row that still has descendants … with a `(+N)` count … Open a row to see that subagent's siblings and direct children with a path back to `main`" (sub-agents docs, "Let subagents spawn their own subagents").

### Hand-off protocol — prompt-only by default; fork as the history mode

- Non-fork subagent initial context = its own system prompt (from the definition) + **the delegation prompt Claude writes** + CLAUDE.md hierarchy + a git-status snapshot taken at parent session start + preloaded skill content + a **sibling roster** (names of other agents in the session, a valid `to` for `SendMessage`) (sub-agents docs, "What loads at startup"; Agent SDK docs, "What subagents inherit" — "The only content you pass from parent to subagent is the Agent tool's prompt string").
- **Fork** (Agent tool type `fork`, or `/subtask`): "A fork is a subagent that inherits the entire conversation so far instead of starting fresh … same system prompt, tools, model, and message history … shares the parent's prompt cache … A fork can't spawn further forks" (sub-agents docs, "Fork the current conversation"). Fork mode is on by default in interactive sessions (v2.1.232+).
- **Result hand-back**: the parent receives the subagent's final message as the tool result, but Claude Code first **scans the final report for instruction-shaped patterns** (imitated `<system-reminder>` tags, `Human:`/`Assistant:` turn markers, permission-config mentions) and neutralizes/marks them — an explicit child-output-is-untrusted boundary (sub-agents docs, "Subagent output scanning"; Agent SDK docs, "What subagents inherit").
- Definitions: markdown files with YAML frontmatter (`name`, `description`, `tools`, `model`, plus `maxTurns`, `background`, `omitClaudeMd`, `isolation`, `skills`, `hooks`, …) in managed > `--agents` CLI > project `.claude/agents/` > user `~/.claude/agents/` > plugin scopes, with recursive discovery and file watching (sub-agents docs, "Configure subagents"). Built-ins: `Explore`, `Plan`, `general-purpose` (+ helpers); "Explore and Plan are one-shot and return no agent ID".

### Concurrency & observation — foreground/background runs, panel tree, completion notifications

- Foreground subagents block the main conversation; background subagents run concurrently, surface every permission prompt in the main session (naming which subagent is asking), and "a background subagent's results reach Claude as a completion notification in a later turn" (sub-agents docs, "Run subagents in foreground or background"). The user can Ctrl+B a running task to background it, or `x` to stop a row.
- Concurrency cap: `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` default 20 — spawning the 21st fails with `Concurrent subagent limit reached`; spend cap `maxBudgetUsd` refuses new spawns, stops running background subagents, and ends the query with `error_max_budget_usd` (Agent SDK docs, "Cap subagent depth, concurrency, and spend").
- The tool itself: appears as `"Agent"` in `tool_use` blocks but as `"Task"` in the `system:init` tools list (before v2.1.63 tool_use was also `"Task"`); messages from inside a subagent's context carry `parent_tool_use_id`, which is exactly the field a GUI needs to attribute streamed events to a tree node (Agent SDK docs, "Detect subagent invocation").

### Lifecycle — spawn / complete+id / stop / resume-by-message

- Spawn = `Agent` tool call with `subagent_type` (optional `name`, `isolation: "worktree"` for file-edit isolation).
- Complete: the Agent tool result includes an `agentId: <id>` text trailer (absent for one-shot Explore/Plan); hitting `maxTurns` marks the output **partial** with a note that it can be messaged to continue (Agent SDK docs, "Resume subagents").
- **Resume = message**: Claude (or a subagent with the tool) uses the `SendMessage` tool with the agent ID/name; "the subagent resumes in the background without a new `Agent` invocation … retains its full conversation history … Resuming starts a new run of the agent under the same ID"; a user-stopped subagent refuses auto-resume (message is refused, "the agent was cancelled"); name reuse is guarded (v2.1.199 check) (sub-agents docs, "Resume subagents").
- **Kill**: `TaskStop` tool / `x` in the panel; background subagents can be stopped and their results lost, or messaged later (as above).
- Auto-compaction applies inside subagent transcripts (`compact_boundary` events logged in the child JSONL) (sub-agents docs, "Auto-compaction").

---

## 3. Mastra (agents + workflows, `@mastra/core`)

Mastra's model is the most "backend service"-shaped of the four: subagents are in-process agent invocations exposed as tools, with memory-thread scoping instead of first-class child sessions; separately, Workflows are durable, snapshot-resumable step pipelines.

### Session topology — ephemeral memory threads per delegation, not browsable sessions

- Subagents are added to a parent `Agent` via the `agents` property and "called … the parent agent uses its instructions and each subagent's `description` to decide when and how to delegate" — each becomes a tool named `agent-<id>` (subagents.mdx; tools.mdx example). The `SubAgent` interface is the contract an implementation (e.g. `Agent`) must satisfy (`packages/core/src/agent/subagent.ts`).
- Each delegation gets a **fresh, unique thread**: `subAgentThreadId = inputData.threadId ? `${inputData.threadId}-${randomUUID()}` : generateId({ idType: 'thread', source: 'agent', entityId: agentName, resourceId })`, and resource `${parentResource}-${agentName}` (`packages/core/src/agent/agent.ts`, subagent tool execution). Docs: "Fresh thread per invocation: Each delegation uses a unique thread ID, ensuring clean separation" (subagents.mdx, "Memory isolation").
- These delegation threads are deliberately invisible: "Ephemeral subagent delegation threads are never surfaced … suppress title generation" (`agent.ts`, same region; ref. issues #22217/#18738). So unlike OpenCode/Claude Code, the subagent "session" is a scoped memory record, not a user-navigable session object.
- **Workflows** are the first-class durable units: typed `createStep`s composed by `createWorkflow`, with branching, parallel, loops; "A snapshot is a serializable representation of a workflow's complete execution state … captured … whenever a workflow is suspended, and persisted to the configured storage system" (workflows/overview.mdx; workflows/snapshots.mdx).

### Hand-off protocol — full parent history by default (the outlier), filterable

- "By default, subagents receive the **full conversation context** from the parent agent. Use `messageFilter` to control what messages are shared" — the callback gets `{ messages, primitiveId, prompt }` and returns a filtered array (subagents.mdx, "Message filtering"). This is a *session-slice* hand-off, the opposite of Claude Code's prompt-only default.
- Memory isolation compensates: "only their specific delegation prompt and response are saved to their [own] memory" (subagents.mdx, "Memory isolation"); subagents without their own memory are injected the supervisor's memory with `lastMessages: false` (`agent.ts`).
- Result back to parent = subagent's text response; "Nested tool calls and subagent metadata, such as thread and resource IDs, aren't added to the parent agent's model context" unless `includeSubAgentToolResultsInModelContext: true` (subagents.mdx, "Subagent result context").
- `enableResultReferences` gives each non-empty successful result a ref ID (`[ref: explorer-1]`) and a `contextFromRefs` input so later delegations can reuse an earlier result **verbatim** instead of the parent restating it (subagents.mdx, "Reusing an earlier subagent result").
- Delegation is interceptable: `onDelegationStart` (proceed/reject/`modifiedPrompt`/`modifiedMaxSteps`, and pass values across the boundary via the request context), `onDelegationComplete` (`feedback` persisted to parent memory, `resultText` to replace what the parent model sees, `bail()` to stop the parent loop), `onIterationComplete` (feedback/stop), all with a `hookErrorStrategy` and hook-failure recording (subagents.mdx, "Delegation hooks").
- **Task completion scoring**: `isTaskComplete` with scorers after each iteration, including the built-in rubric scorer — an LLM-as-judge that checks a checklist and feeds failing criteria back into the conversation until satisfied or `maxSteps` (subagents.mdx, "Task completion scoring").

### Concurrency & observation — in-process; background tasks, approval propagation, abort forwarding

- Subagent invocations are tool calls, so they can run as **background tasks** (`backgroundTasks` manager + per-tool opt-in with `timeoutMs`); use `streamUntilIdle()` "so the stream stays open until the subagents complete and the parent agent has had a chance to respond to their results" (subagents.mdx, "Running subagents in the background"; docs/harness/background-tasks).
- "Tool approvals propagate through the delegation chain. When a subagent uses a tool with `requireApproval: true` or calls `suspend()`, the approval request surfaces in the parent agent's stream" as a `tool-call-approval` chunk (subagents.mdx, "Tool approval propagation").
- Cancellation: "When you pass an `abortSignal` to the parent agent's stream()/generate(), Mastra forwards that same signal to delegated subagents … cancels in-flight subagent runs at their next step" (subagents.mdx, "Cancellation").

### Lifecycle — spawn(in-process) / complete / abort / suspend+snapshot-resume

- No "resume a finished subagent conversation" primitive: resume in Mastra is a **durable-workflow** concept — `suspend()` at a step saves a snapshot; "resume from a specific step ID to restore the exact captured state … persist across deployments and application restarts" (workflows/suspend-and-resume.mdx).
- The one subagent-level resume nuance: a *suspended delegation* must "continue on the thread/resource pair the suspended run persisted, not freshly generated ones" — the resume path backfills the original pair from the run snapshot (`agent.ts`, "A resumed delegation must continue on the thread/resource pair…").
- Workflows-as-tools: an agent's `workflows` property exposes a workflow as a `workflow-<name>` tool, so a parent agent can delegate a whole durable pipeline as one call (tools.mdx example).

---

## 4. OpenCode (nested session tree)

OpenCode is the clearest counterpoint: sub-agent work is a **first-class child session in a navigable session tree**, not a hidden tool call.

### Session topology — parent → child sessions, navigable in the UI

- "There are two types of agents in OpenCode; primary agents and subagents … Subagents are specialized assistants that primary agents can invoke for specific tasks" (opencode.ai/docs/agents, "Types").
- "When subagents create child sessions, use `session_child_first` (default `<Leader>+Down)` to enter the first child session from the parent. Once you are in a child session, use `session_child_cycle` (Right/Left) … and `session_parent` (Up) to return to the parent session. This lets you switch between the main conversation and specialized subagent work" (opencode.ai/docs/agents, "Usage"). So the subagent *is* a child session, and the navigation model is an explicit tree: enter child, cycle siblings, return to parent.

### Hand-off protocol — description-routed task tool call, @-mention override

- Built-in subagents: **General** ("full tool access (except todo) … to run multiple units of work in parallel"), **Explore** (fast read-only codebase exploration), **Scout** (read-only external-docs/dependency research); primary agents **Build** (all tools) and **Plan** (edits/bash restricted) (opencode.ai/docs/agents, "Built-in").
- Invocation: "Automatically by primary agents for specialized tasks based on their descriptions. Manually by @ mentioning a subagent in your message" (opencode.ai/docs/agents, "Usage") — the same description-matching hand-off as Claude Code.
- Definitions: `opencode.json` `agent` entries or markdown files with frontmatter (`description`, `mode: subagent`, `model`, `prompt`, per-tool `permission`) in `~/.config/opencode/agents/` (global) or `.opencode/agents/` (project) (opencode.ai/docs/agents, "Configure").

### Concurrency & observation — parallel child sessions you can walk

- Parallelism is expressed by the General subagent ("Use this to run multiple units of work in parallel") and is observed by *walking the session tree* — every unit of sub-agent work is a session a user can enter, in contrast to pi-cohort's status polling and Claude Code's panel rows (opencode.ai/docs/agents, "Built-in"/"Usage").
- Hidden system agents (title, summary, compaction) run as primary-mode sessions "not selectable in the UI" (opencode.ai/docs/agents, "Built-in") — housekeeping is first-class sessions too.

### Lifecycle

- Child sessions persist as part of the session tree (they are navigable, so they outlive the invoking turn); the docs page documents no explicit kill/resume API — lifecycle management is session-tree navigation plus per-agent permissions (opencode.ai/docs/agents). (Weaker lifecycle semantics than pi-cohort/Claude Code; noted as the trade-off of the most "visible" model.)

---

## Comparison

| Dimension | pi-cohort 6.1.0 | Claude Code (sub-agents docs, v2.1.2xx) | Mastra (@ 317faeb2) | OpenCode (docs) |
|---|---|---|---|---|
| Topology | Separate child **process + linked session file** (fresh or branched via `createBranchedSession`) | Separate context **within** the session; transcript as linked `subagents/agent-{id}.jsonl`; nested tree up to depth 3 | In-process invocation; **ephemeral memory thread** per delegation (never surfaced); Workflows are the durable unit | **Child sessions in a navigable tree** (`session_child_*`/`session_parent`) |
| Hand-off | Compact contract prompt + `defaultReads` + optional file artifacts / `{previous}` / named outputs / `outputSchema`; history **not** inherited | Prompt-only (own system prompt + task message + CLAUDE.md + git snapshot + skills + sibling roster); **fork** = full history + shared cache | **Full parent history by default**, `messageFilter` to narrow; text result only back; `[ref: …]` result references | Description-routed task tool call (auto or @-mention); child starts its own session |
| Observation | Status **tree** (`action:"status"`), `WorkflowGraphSnapshot` with phases, `needs_attention` events, completion delivery, `Σ$` cost rollup, optional `monitor` child | Subagent **panel** rows + tree (`(+N)`), transcript drill-in, completion notification in a later turn, permission prompts surfaced in main session, `parent_tool_use_id` on every child message | `streamUntilIdle` (stream stays open until children finish), approval chunks propagated up, abort forwarded | Walk in/out of child sessions in the TUI |
| Lifecycle | Spawn=child process; complete/failed/paused/queued/running; **interrupt→paused**; **resume=revive** new process from persisted session file; depth cap 2; worktree isolation | Spawn=`Agent` tool; complete returns `agentId`; **resume=`SendMessage`** (new run, same ID, full history); stop=`TaskStop`/`x`; partial on `maxTurns`; concurrency 20; depth 3; budget cap | Spawn=in-process tool; complete=tool result; **kill=abort** at next step; **suspend/snapshot/resume** (durable, across restarts); approval propagation | Child sessions persist in tree; no documented kill/resume API |
| Acceptance model | Explicit `acceptance` levels (attested/checked/verified/reviewed) + machine evidence kinds; `BLOCKED:` stop protocol | `maxTurns` partial marker; output scanning; no explicit acceptance contract | `isTaskComplete` scorers (incl. LLM-as-judge rubric); iteration feedback loops | None documented |

## Synthesis — what a native tau-core + GUI should borrow

The four systems converge on a small stable core and diverge on a few deliberate choices. Recommended tau model, item by item:

1. **Model a sub-agent as a real session entity with its own file** (topology = separate linked session, not an in-memory blob). pi-cohort's branch-from-parent-session-file (`fork-context.ts`) and Claude Code's `subagents/agent-{id}.jsonl` both show the same storage pattern: the child's transcript is a first-class file linked to the parent session's directory tree. This gives resume, compaction-survival, and GUI drill-in for free.
2. **Two explicit context modes**: `fresh` (default — prompt-only contract + artifacts, per pi-cohort/Claude Code) and `fork` (branched copy of parent history, per pi-cohort `createBranchedSession` / Claude Code `fork`). **Do not default to full-history inheritance** — Mastra's default is the outlier and the most expensive hand-off; if needed, expose a `messageFilter`-style narrowing hook rather than a different default.
3. **One opaque handle, three operations.** Claude Code's protocol is the cleanest: spawn returns an `agentId`; `SendMessage(handle)` resumes (new run, same identity, full history); `TaskStop(handle)` kills. pi-cohort's run-id + index + nested-id targeting adds multi-child addressing. tau-core spawn should return such a handle, and resume/stop/inspect all take it.
4. **Resume = revive-from-file, never re-run.** pi-cohort's `buildRevivedAsyncTask` ("You are reviving a previous subagent conversation … Use the stored session context as background") and Claude Code's "resuming starts a new run of the agent under the same ID" are the same idea: persistence is the session file, revival is a new process/turn on it. tau's resume semantics should be defined this way, including the "running child cannot be resumed" and "no persisted file → fail" failure modes.
5. **Push completions; don't make parents poll.** pi-cohort ("Pi will deliver the async completion"), Claude Code ("results reach Claude as a completion notification in a later turn"), and Mastra (`streamUntilIdle` + propagated approval chunks) all push. tau-core should emit child lifecycle events (spawned / needs-attention / paused / complete / failed) into the parent session stream, with `needs_attention`-style inactivity signals as an event, not a status (pi-cohort's explicit "Attention signals are not lifecycle state" is a good invariant).
6. **Bounded tree, enforced at the runtime.** Borrow the two independent caps: max depth (pi-cohort 2, Claude Code 3 — pick a default, env-overridable) and max concurrent children (Claude Code 20 / pi-cohort 4-per-group). Plus the depth-limit behavior: at the limit, children simply don't get the spawn tool.
7. **GUI: render the session tree with pi-cohort's graph snapshot as the data model.** `WorkflowGraphSnapshot` (nodes: seq step / parallel group / dynamic child; status; phase; label; `acceptanceStatus`; error; parent path back to `main`) plus Claude Code's presentation rules (one row per node, `(+N)` descendant counts, click to open the child transcript, keep failed rows visible briefly, footer hint to a tasks view) plus OpenCode's navigation (enter child / cycle siblings / return to parent as first-class keys). The child transcript view should show `parent_tool_use_id`-style provenance on every message (Claude Code) so the tree is auditable.
8. **Treat the boundary in both directions.** Outbound: strip parent-only orchestration context and injected orchestration instructions from the child's inherited material (pi-cohort `rewriteSubagentPrompt` / `PARENT_ONLY_CUSTOM_MESSAGE_TYPES`); the child gets a role-specific boundary instruction. Inbound: mark/scan the child's final output as untrusted before the parent's model reads it (Claude Code output scanning), and propagate approval/permission requests up to the parent UI (Mastra `tool-call-approval`, Claude Code background permission surfacing) rather than auto-denying or auto-approving.
9. **Hand-off artifacts are files.** pi-cohort's per-run `input.md`/`output.md`/`_meta.json`/`.jsonl` artifact dir, `outputMode: "file-only"` compact references, and Mastra's `[ref: explorer-1]` result references show the same pattern: large results become durable, referenceable artifacts; only compact summaries travel in context. tau's "task" hand-off should be defined as (compact prompt contract) in, (final message + optional artifact references) out — with structured-output schemas (pi-cohort `outputSchema`) for machine-consumable results.
10. **Acceptance as a first-class run attribute.** pi-cohort's `acceptance` levels (attested/checked/verified/reviewed) with enumerated evidence kinds, plus Mastra's `isTaskComplete` scorers/rubric as the "verified by an independent check" layer, plus the `BLOCKED:` stop protocol for unapproved decisions. tau tasks should carry an acceptance level and the GUI should surface per-node `acceptanceStatus` (already a field in pi-cohort's graph snapshot).
11. **Keep "task pipeline" and "sub-agent" as two primitives.** Mastra is the only one to draw the line explicitly: subagents (open-ended, in-process, ephemeral threads) vs Workflows (deterministic typed steps, `suspend()`/snapshot/resume across processes, Studio visualization); pi-cohort's "chain" sits deliberately in between (saved `.chain.md`/`.chain.json` sequences with fan-out/fan-in, still agent runs). tau-core should offer: sub-agent (session entity, free-running) and task chain (declared sequence/parallel/expand with per-step contract), not one conflation.
12. **Write safety: single-writer default.** pi-cohort's "keep writes single-threaded by default … parallelize reading, review, validation, synthesis, not normal writes" with `worktree: true` as the opt-in isolation, and Claude Code's `isolation: "worktree"` on forks, are the same guard. Any tau parallel-writer mode should be an explicit worktree/isolation opt-in, never the default.

**Pitfalls noted in the wild** (worth encoding as tau constraints): user-stopped children must refuse auto-resume (Claude Code); name/ID reuse must be checked, not trusted (Claude Code v2.1.199); resumed subagents must not receive fresh thread IDs that break snapshot restore (Mastra #22217); subagent output scanning exists precisely because children read untrusted web/file content (Claude Code); and deep fan-out of *writers* is the failure mode both pi-cohort (staged fix orchestration: parallel planners → one writer → parallel validators) and the ticket's own acceptance contract are designed around.
