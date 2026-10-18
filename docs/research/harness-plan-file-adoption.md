# Plan-file / todo adoption in other agent harnesses (2026-10)

**Status**: research notes (2026-10-18) · **Provenance**: primary sources — npm bundles, GitHub source trees, official docs, first-party engineering blogs. Every claim carries its source. Closed-source items (Cursor, Devin) are marked with what their docs do and don't disclose.

## Question

How do other agent harnesses make the model *adopt* a plan or todo artifact — when it is created, what shape it takes (file vs. in-context tool state), how it is kept fresh, and how it is re-injected into context? The goal is a comparison against Tau's plan-file mechanism and the five candidate adoption levers (gate enforcement, concrete "when" rules, seeding with real content, per-turn next-step nudge, subagent contract).

## Short answer

Nobody in the open-source set uses a plan *file* as the live progress artifact — except Manus, which is the outlier that most closely matches Tau. Claude Code, Codex, Cline, Gemini CLI, and Roo Code all track progress through a **tool** (`TodoWrite`, `update_plan`, `write_todos`, `update_todo_list`) whose state lives in the message stream, not on disk. A separate, smaller family treats planning as a **mode** with a file or chat block as its *deliverable* (Cursor's editable plan markdown, Gemini CLI's `exit_plan_mode` plan path, Cline/Codex's `<proposed_plan>` block). Manus is the only one that writes and re-reads a `todo.md` in the sandbox, and does so *deliberately as an attention mechanism*, not for persistence. The strongest adoption levers in the wild: explicit status vocabularies with a single-`in_progress` invariant, threshold-based staleness reminders (Claude Code), a read-only gate on the planning mode (Cline/Codex/Gemini), and recitation of the current state near the end of context (Manus).

## 1. Claude Code — `TodoWrite` as in-context tool state, with a staleness nudge

Source: npm bundle `@anthropic-ai/claude-code@2.0.77` (`package/cli.js`, the last release shipped as a JS bundle; 2.1.x is a native-binary stub with no extractable prompts).

**The tool.** `TodoWrite`'s description: "Update the todo list for the current session. To be used proactively and often to track progress and pending tasks. Make sure that at least one task is in_progress at all times. Always provide both content (imperative) and activeForm (present continuous) for each task." The input schema is a `todos` array of `{content, status, activeForm}`; the tool returns both `preTodos` and `newTodos` ("The todo list before/after the update") — so the *entire current list* is re-stated in every tool result, which keeps the live plan at the end of the visible history for free. Declared `strict: true`, `isConcurrencySafe: false`.

**The "when" rules.** The system prompt's "Task Management" section (only present when `TodoWrite` is in the tool set) instructs *very frequent* use and says to mark todos completed *immediately, not batched* — with two worked examples (build + fix type errors; feature implementation with metrics). This is the most explicit "when to touch the plan" guidance in the whole set.

**The staleness nudge.** A threshold pair `TURNS_SINCE_WRITE: 10, TURNS_BETWEEN_REMINDERS: 10` drives an injected attachment (`type: "todo_reminder"`) with the text: "The TodoWrite tool hasn't been used recently. If you're working on tasks that would benefit from tracking progress, consider using the TodoWrite tool to track progress. Also consider cleaning up the todo list if has become stale and no longer matches what you are working on. … This is just a gentle reminder - ignore if not applicable. Make sure that you NEVER mention this reminder to the user." Note the dual duty: nudge to *update* and nudge to *prune*.

**Adoption verdict.** Adoption is prompt-driven + a mechanical reminder; there is no file, no gate, no mode. The whole plan lives in the tool results of the message stream, so it compacts away with history.

## 2. Codex CLI — `update_plan` tool, plus a distinct Plan *mode* that produces a chat block

Sources: [`codex-rs/core/src/tools/handlers/plan_spec.rs`](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/plan_spec.rs), [`codex-rs/prompts/src/update_plan_instructions.rs`](https://github.com/openai/codex/blob/main/codex-rs/prompts/src/update_plan_instructions.rs), [`codex-rs/core/gpt_5_2_prompt.md`](https://github.com/openai/codex/blob/main/codex-rs/core/gpt_5_2_prompt.md), [`codex-rs/collaboration-mode-templates/templates/plan.md`](https://github.com/openai/codex/blob/main/codex-rs/collaboration-mode-templates/templates/plan.md).

**The tool.** `update_plan` takes an optional `explanation` plus a required `plan` array of `{step, status: "pending"|"in_progress"|"completed"}`; description: "Updates the task plan. … At most one step can be in_progress at a time." No output schema. The model prompt carries two sections: `## Planning` (when to use it) and `## update_plan` (step hygiene: 5–7 words per step, exactly one `in_progress`).

**The mode.** Plan Mode is a *collaboration-mode template*, not a file. It runs 3 phases — "Ground in the environment (explore first, ask second)", "Intent chat", "Implementation chat" — and finalizes with a `<proposed_plan>` block in chat. The plan must be "**decision complete**, where the implementer does not need to make any decisions." Mode rules are strict: "Plan Mode is not changed by user intent, tone, or imperative language." Non-mutating exploration is allowed; mutating actions are not. The prompt explicitly separates the two: "Separately, `update_plan` is a checklist/progress/TODOs tool; it does not enter or exit Plan Mode. … If you try to use `update_plan` in Plan mode, it will return an error."

**Gate enforcement, mechanically.** `without_update_plan_instructions()` *strips* the checklist-tool guidance sections ("## Planning", "## update_plan", "## Plan tool", "## Plan Mode vs update_plan tool", the "Progress visibility:" line, and matching bullets) from Codex-owned prompts before appending caller text — i.e., the harness, not the model, decides whether plan-tool guidance exists in a given context.

**Adoption verdict.** In-context tool state for progress; a chat block (not a file) for the plan deliverable; the strongest *mode gate* in the set, enforced in code (tool returns an error) not just in prompt.

## 3. Cline — Plan & Act dual mode; the todo tool is deliberately *not* the plan

Sources: [`docs/core-workflows/plan-and-act.mdx`](https://github.com/cline/cline/blob/main/docs/core-workflows/plan-and-act.mdx), [`apps/vscode/src/core/prompts/responses.ts`](https://github.com/cline/cline/blob/main/apps/vscode/src/core/prompts/responses.ts), [`sdk/packages/core/src/tasks/agenda-task-tool.ts`](https://github.com/cline/cline/blob/main/sdk/packages/core/src/tasks/agenda-task-tool.ts).

Plan mode is a 3-phase collaboration identical in spirit to Codex's (ground in environment → intent chat → implementation chat), ending in a `<proposed_plan>` block. Plan mode allows non-mutating exploration and forbids mutating actions; the agent **cannot self-switch to Act mode** — the user must accept the plan (it interacts via a `plan_mode_respond` tool). Like Codex, Cline separates the mode from the checklist: using `update_plan` while in Plan Mode returns an error. The SDK's `agenda-task-tool.ts` (329 lines) is the todo/checklist tool, with a `mode` field enum `["act", "plan", "yolo"]`. A `/deep-planning` slash command and separate model-per-mode configuration round out the system.

**Adoption verdict.** Same shape as Codex: chat-block plan, in-context todo tool, hard mode gate in code.

## 4. Gemini CLI — the one open-source tool with a *required plan file path*

Sources: [`docs/cli/plan-mode.md`](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/plan-mode.md), [`docs/tools/planning.md`](https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/planning.md), [`packages/core/src/tools/write-todos.ts`](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/write-todos.ts), [`packages/core/src/tools/definitions/model-family-sets/default-legacy.ts`](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/tools/definitions/model-family-sets/default-legacy.ts).

**Plan Mode with a file.** `enter_plan_mode` (optional `reason`) switches the approval mode to `PLAN`: read-only tools allowed (`read_file`, `list_directory`, `glob`, `grep_search`, web tools); `write_file`/`replace` permitted **only for `.md` files in the plans directory**. Plans live in `~/.gemini/tmp/<project>/<session-id>/plans/` by default; a custom directory is configurable via `settings.json` → `general.plan.directory`, restricted to the project root, and enforced through the policy engine (TOML `argsPattern` rules). `exit_plan_mode` **requires a `plan_path`** that must sit in that plans dir — the file is a *required output of the planning phase*, presented for formal approval; the agent "MUST reach an informal agreement … BEFORE calling this tool." Not available in YOLO mode.

**The todo tool.** `write_todos` is a full-list replacement tool with a five-state vocabulary: `pending`, `in_progress`, `completed`, `cancelled`, `blocked`. Its description: "Use this tool for complex queries that require multiple steps. DO NOT use this tool for simple tasks that can be completed in less than 2 steps." — a *threshold* adoption rule in the tool's own description.

**Adoption verdict.** The closest open-source analogue to Tau's file-based plan: the file path is a hard, code-enforced contract of `exit_plan_mode`, the planning mode is read-only-gated, and the todo tool carries an explicit "when" threshold.

## 5. Roo Code — full-list markdown replacement in a todo tool

Sources: [`src/core/prompts/tools/native-tools/update_todo_list.ts`](https://github.com/RooCodeInc/Roo-Code/blob/main/src/core/prompts/tools/native-tools/update_todo_list.ts), [`apps/docs/docs/advanced-usage/available-tools/update-todo-list.md`](https://github.com/RooCodeInc/Roo-Code/blob/main/apps/docs/docs/advanced-usage/available-tools/update-todo-list.md).

`update_todo_list` takes a single `todos` string — a markdown checklist — with statuses `[ ]` pending, `[x]` completed, `[-]` in progress; single-level only, full-list replacement. This is the one tool in the set whose payload *is* markdown checklist syntax (the closest textual cousin to Tau's plan-file format), but it lives in tool state, not a file. The feature docs frame it as a UI-surfaced progress list, not a plan deliverable.

## 6. Cursor — plan mode whose deliverable is an editable markdown file

Source: [cursor.com/docs/agent/plan-mode](https://cursor.com/docs/agent/plan-mode) (closed-source; docs only).

"Plan Mode creates detailed implementation plans before writing any code. Agent researches your codebase, asks clarifying questions, and generates a reviewable plan you can edit before building." Key detail: "You review and edit the plan through chat **or markdown files** … Plans are saved by default in your home directory. Click 'Save to workspace' to move it to your workspace for future reference, team sharing, and documentation." So Cursor does treat the plan as a first-class markdown file artifact, but it is a **one-shot deliverable** of the planning phase — a human-reviewable spec, not a per-turn-updated progress ledger. The docs disclose no re-injection or staleness mechanism (closed-source).

## 7. Windsurf / Cascade — a background planning agent plus a todo list

Source: [docs.windsurf.com — Cascade overview, "Plans and Todo Lists"](https://docs.windsurf.com/windsurf/cascade/cascade).

"In the background, a **specialized planning agent** continuously refines the long-term plan while your selected model focuses on taking short-term actions based on that plan." Cascade maintains a Todo list in the conversation for complex tasks; the user edits the plan by asking, and "Cascade may also automatically make updates to the plan as it picks up new information, such as a Memory, during the course of a conversation." No file; the split between a long-horizon planning agent and a short-horizon acting model is the distinctive design point — planning is offloaded to a separate model instance rather than the same loop.

## 8. Manus — the outlier: `todo.md` as an attention mechanism

Source: [Manus, "Context Engineering for AI Agents: Lessons from Building Manus"](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus) (Yichao "Peak" Ji, 2025-07-18).

"When handling complex tasks, it tends to create a `todo.md` file—and update it step-by-step as the task progresses, checking off completed items. That's not just cute behavior—it's a deliberate mechanism to manipulate attention." The stated rationale: "A typical task in Manus requires around 50 tool calls on average. … By constantly rewriting the todo list, Manus is **reciting its objectives into the end of the context**. This pushes the global plan into the model's recent attention span, avoiding 'lost-in-the-middle' issues and reducing goal misalignment." Two design properties to note: (1) the todo file is *rewritten* (not appended to) so the current state sits at the end of context — exactly the property Tau's per-turn re-injection block buys mechanically; (2) the same post's "Use the File System as Context" section frames the filesystem as "unlimited in size, persistent by nature, and directly operable by the agent itself," with all compression designed to be *restorable* (drop the web page from context if the URL is preserved). Manus pairs the todo file with that general file-as-externalized-memory doctrine.

## 9. Aider — planning as a two-model mode, no artifact

Sources: [`aider/coders/architect_prompts.py`](https://github.com/Aider-AI/aider/blob/main/aider/coders/architect_prompts.py), [`aider/coders/architect_coder.py`](https://github.com/Aider-AI/aider/blob/main/aider/coders/architect_coder.py).

Aider's "architect" mode is the set's cleanest two-model split: the architect prompt says "Act as an expert architect engineer and provide direction to your editor engineer. … The editor engineer will rely solely on your instructions, so make them unambiguous and complete." The mechanism: when the architect's reply completes, `reply_completed()` instantiates a **fresh editor coder** and passes the architect's message as its task prompt (`editor_coder.run(with_message=content)`), then folds the result back as "I made those changes to the files." No todo file, no plan file — the "plan" is a single chat message that becomes the second model's initial user prompt. Worth citing as the boundary case: plan-as-message, consumed exactly once.

## 10. SWE-agent — static numbered steps in the template, no live plan

Source: [`config/default.yaml`](https://github.com/SWE-agent/SWE-agent/blob/main/config/default.yaml).

The `instance_template` embeds a fixed five-step procedure ("1. find and read code relevant … 2. Create a script to reproduce the error … 3. Edit the sourcecode … 4. Rerun your reproduce script … 5. Think about edgecases") directly in the task prompt. There is no tool, file, or state: the "plan" is static text, identical for every task, and never updated. The useful contrast: SWE-agent optimizes for a single-issue benchmark trajectory where a fixed recipe beats a live plan; nothing in its config updates step state.

## 11. Devin (Cognition) — what the docs do and don't say

Sources: [docs.devin.ai — AGENTS.md](https://docs.devin.ai/onboard-devin/agents-md), [Dynamic Workflows](https://docs.devin.ai/work-with-devin/dynamic-workflows) (closed-source agent; docs only).

Devin's public docs expose no plan-file mechanism. What they do disclose: `AGENTS.md` is auto-injected with a **16 KiB cap from the beginning of the file**, with a truncation notice and on-demand full-file reads beyond it — a direct precedent for "critical instructions near the top; long situational guidance in referenced files." "Dynamic Workflows" (a deterministic Python script that orchestrates many Devin sessions, with every agent call recorded so a run is resumable — "completed agents replay their recorded results instantly, and only the unfinished work runs again") is the nearest thing to an externalized plan, but it is a *human/agent-authored orchestration script*, not an agent-maintained progress ledger. Treat any claim about Devin's internal planning as undocumented.

## Synthesis — what this means for Tau's five levers

The open field splits into two shapes: **tool-state progress** (Claude Code, Codex, Cline, Gemini CLI, Roo) and **file/deliverable plans** (Cursor, Gemini CLI's plans dir, Manus, plus Tau). Tool-state plans are cheap and always-at-end-of-context but compaction-fragile; file plans survive compaction and session re-open at the cost of needing an adoption mechanism. Tau's plan file is in the second, smaller camp — and the five candidate levers map onto verified practices in that camp:

**1. Gate enforcement.** Strongest precedent: Cline/Codex (Plan Mode forbids mutating actions; `update_plan` in plan mode *returns an error* — enforced in code, not prompt) and Gemini CLI (plan mode restricts `write_file`/`replace` to `.md` files in the plans dir via policy-engine `argsPattern` rules). Tau's equivalent: while a plan exists with unchecked steps, the harness already knows the state; a gate on mutating tools before first step completion is the one lever with a direct, code-enforced analogue. Manus offers the counterweight: no gate at all — adoption by recitation only.

**2. Concrete "when" rules in the injected section.** The set's best "when" guidance is specific and threshold-based: Claude Code's "mark completed immediately, no batching" + worked examples; Gemini CLI's "DO NOT use for tasks completable in <2 steps" *inside the tool description*; Codex's "5–7 words per step, exactly one `in_progress`." Generic "keep the plan up to date" language appears nowhere. The injected plan section should carry status-invariant and update-frequency rules in that concrete form.

**3. Seeding with real content.** Nobody seeds an empty skeleton. Every harness that produces a plan produces a *complete* one at creation: Codex/Cline require a "decision complete" `<proposed_plan>`; Gemini CLI's `exit_plan_mode` requires a populated `plan_path`; Cursor generates a "comprehensive implementation plan"; Manus creates `todo.md` with the task's real steps. Tau's create-if-missing skeleton (Steps/Criteria/Notes headings only) is unique in the set — and unique in being *empty*. The consistent external pressure is: seed with the model's actual first plan (e.g., after the first turn, or on the first plan-file write), not a bare scaffold.

**4. Per-turn next-step nudge in the re-injected block.** Two independent precedents converge on Tau's lever. Claude Code's threshold pair (`TURNS_SINCE_WRITE: 10`, `TURNS_BETWEEN_REMINDERS: 10`) is a mechanical staleness reminder with an explicit "never mention this to the user" envelope. Manus's recitation argument is the *why*: rewriting the todo keeps the global plan "in the model's recent attention span," defeating lost-in-the-middle. Tau re-injects the whole plan every turn (cheap because it's short), so the marginal addition is one line of *direction* — "next unchecked step: …" (which `resume_contract` already computes) — turning a state dump into a nudge, with Claude Code's envelope as the model for keeping it unobtrusive.

**5. Subagent contract.** The closest analogues are Aider's architect→editor handoff (plan-as-message consumed exactly once by a fresh coder) and Manus's file-as-context doctrine (fresh context rebuilt from filesystem artifacts; the C-compiler project's "READMEs and progress files … updated frequently" from the memory synthesis). Neither is a *shared* live plan between parent and child. Tau's `with_path` (child prompts point at the child's own plan file) plus `resume_contract` on resume is closer to the Aider/Manus pattern than to any shared-file contract in the field — the research finds no harness where a parent and subagent co-edit one plan file, which makes Tau's per-child-plan model unusual but not unsupported.

**What nobody does** (design headroom): prune-on-stale (Claude Code's reminder *suggests* cleaning stale items; no harness auto-prunes), diff-suppressed `PlanChanged` events (a Tau-specific watcher concern, no external analogue), and any *negative* test of plan adherence in CI/eval. The adoption literature is one-directional — make the model touch the artifact — and none of the five levers has a documented failure mode to defend against, which is itself a finding.
