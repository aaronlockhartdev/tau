# Plan-file & subagent model (design)

Status: **agreed** (2026-10-15). Consolidates the design discussion; supersedes the task
half of ADR-0001 and rewrites spec §5.2/§5.3/§5.4. Related: #55 (decision), #56 (gates),
#51 (research), #53 (background jobs). SOTA backing at the foot.

## Summary

The 7-tool task state machine is **dropped**. In its place, one system:

- a session-scoped **plan file** the model edits with its ordinary file tools — the **source
  of truth** for the work (the core is its only machine reader);
- a **uniform, mechanical, user-configured completion gate** (build + test suite) — the
  **oracle**, run `enforce` always;
- a **machine-observed subagent lifecycle** — a child's state is *read* from observed
  signals, not *declared*;
- the invariant that a **child holds no lock on its parent** — all child→parent effects are
  values the parent ingests.

The through-line: **the model does work and writes files; the core observes and
coordinates.** "Model declares state" gives way to "core observes state."

## The core (v0)

### 1. Plan file — the source of truth
- Path `{workspace}/.tau/plans/{session-id}.md`, injected into the system prompt.
- **Markdown** (not JSON — tamper-safety comes from the gate, not the format): steps as
  checkboxes, a `## Criteria` section, freeform notes.
- Carries **status** (what + progress) — **no per-item check commands** (the earlier in-file
  gates block is dropped; not the practitioner consensus).
- The **core is the only machine reader**: the file watcher (`harness/watchers.rs`) parses on
  change; the parser is a **total function** (checkboxes are the only hard requirement;
  fuzzy section match; unrecognized → notes; never fails, never blocks).
- **Seeded** at session start (initializer pattern); the model keeps it current.
- **Re-injected** into context at each turn boundary and after compaction — the key mechanism
  that makes the model *use* the plan (the #24 resume-contract principle, generalized).
- **GUI: the plan pane** (per session) — the payoff, and the stakeholder-alignment checkpoint.
- The **single source of truth** for the work; the core keeps **no parallel task state**.

### 2. Completion gate — the oracle
- A **global, mechanical, checkable predicate** (build + test suite), configured **once** in
  user config, applied to **all** work units. No per-plan criteria authoring — the test suite
  *is* the per-unit criteria.
- **Uniform + blocking + `enforce` always** (no `off`/`advisory`/`enforce` dial). Runs at
  every qualifying turn-end; no "when to verify" judgment.
- **Turn-end stop-hook**: no `QUESTION:` marker **and** ≥1 tool call → run the check. Pass →
  the turn stands. Fail → a system entry (truncated failure) + a steering-lane injection → a
  repair turn. After `max_retries` → the session is marked `gate_failed` (live state +
  status-bar field + badge).
- The check command is **stated in the system prompt** when configured ("your work is
  verified by: …").
- **The check is authored by the delegator, never the model** — the user for a root session,
  the parent (in the task brief) for a child. An external validator, not self-assessment
  (ADR-0007: a deliberate transparency strengthening).
- **TDD makes it meaningful** (agent-type prompt discipline, not machinery): the model writes
  **spec-derived tests first**. Bug fix → a **fail-to-pass** reproduction test (red on the
  buggy code, green after) — the canonical strong case. New feature → spec-anchored tests; a
  vague spec becomes a **plan-pane alignment checkpoint** (the user confirms what "done" means
  before implementation).

### 3. Machine-observed subagent lifecycle
A child's state is **read**, not declared:

| state | observed from |
|---|---|
| `running` | the child's turn loop is active |
| `idle` | turn ended; not complete; no `QUESTION:` marker — a **safe resting state** (resumable; nothing lost, nothing forced) |
| `done` | plan complete **and** gate passed (observed) |
| `awaiting` | a `QUESTION:` marker in the child's final output |
| `failed` | `gate_failed` after retries, or a turn/provider fault |
| `stopped` | soft interrupt (user/parent) — unchanged |

- The **resting declaration** (`parent_notify {done / waiting_user / waiting_parent}`) is
  **secondary** — a hint observation confirms or overrides. "The model forgets to declare" is
  a non-issue: the core doesn't depend on the declaration existing.
- **`QUESTION:` marker** = the one small convention for awaiting-input.
- **The one-shot nudge is deleted** — an undeclared stop is `idle`, never force-`failed`.
- **Idle-notification routing**: a child informs its delegating parent **only on an
  autonomous pause** (a self-driven turn that came to rest). A rest that is a *reply to a
  direct message* (the user in the tab, or a parent `subagent_message`) is silent to the
  parent — the sender is engaged. Per-turn classification, not a sticky mode.

### 4. Invariants (constraints, not features)
- **A child holds no lock on its parent.** All child→parent effects are *values the parent
  ingests* (a reported plan, a result, a question) — never a write into the parent's state
  under a lock the child took.
- **The parent is the sole actor on its own state.**
- **The plan file is the single source of truth**; the core keeps no parallel task state.
- **Observation over declaration.**
- **Exclusive step ownership** at the delegation boundary (one owner per step; takeover = a
  clean ownership transfer). **No session validates its own work** — each validates the level
  below, is validated by the level above (user → root → children → …).

## What dies / survives
- **Dies**: the 7 task tools, the core task state machine, typed task events, evidence-gated
  completion, the one-shot nudge, the in-file gates block.
- **Survives**: the GUI pane (→ plan pane, new source) · the **resume-contract concept** (→
  the plan's next-action + a session-scoped handoff file) · sub-agent briefs · the child
  lifecycle (re-driven by observed signals) · **direct user↔child** (an ordinary session;
  orthogonal; never routes through the parent).

## Satellites (post-v0 — each its own ticket, independently deferrable)
- **Advisory verifier** — a "reviewer" subagent the core spawns on observed completion; reads
  diff + spec + plan, **returns an advisory value** (non-blocking, cheap model, in-band).
  Uniform + advisory. Distinct from the out-of-band PR review.
- **Staleness nudge** — N tool-bearing turns with no plan edit → a `<system-reminder>`.
- **Fleet projection** — compact child plans in the parent's view.
- **Two-agent contract/impl split** — high-stakes features only (stronger test-independence,
  at the cost of invasiveness).

## Out of core
- **GitHub PR integration / PR-review bots** — user-setup, out-of-band. tau does not touch
  PRs; core verification is the plan file + the mechanical gate.

## Sequencing
- **v0**: the core (§1–4) + the plan pane. **This alone resolves all three original
  problems** — the deadlock (no task store to write across a lock), the fragile declared
  completion (completion is *observed*; the nudge is gone), and the 7-tool state machine
  (replaced by file + gate).
- **post-v0**: the satellites, one ticket at a time, when they earn their place.
- **Discipline**: no speculative code — build the ticket, the next ticket extends; #51
  minimalism (tool surface is an ablation axis).

## Open items
1. ADR-0001 revision (supersedes the task half; re-drives the lifecycle; the no-parent-lock
   invariant).
2. Spec §5.2/§5.3/§5.4 rewrite.
3. ADR-0007 note (enforce-always as a transparency strengthening).

## Risks
- **Flaky checks** are hard blocks (no `advisory` escape) — the user owns check reliability.
- **Turn-end latency** — bounded by `timeout_secs` + `max_retries`.
- **New-feature verification** is intent-strong but mechanically softer than a bug's F2P.

## SOTA backing
- **Strong**: plan file (Anthropic initializer + progress/feature file; Claude Code todos) ·
  completion gate + repair loop (Reflexion 91% HumanEval; Self-Refine; arXiv 2607.14167
  external-validator + bounded loop; Anthropic "marks done without testing") · value-returning
  subagents (Claude Code "only the final message returns"; pi "write only to the output
  path") · TDD/F2P (TDD-Bench Verified, 449 issues; Auto-TDD) · uniform-advisory AI review
  (review-bot practice: "advisory, never blocking").
- **Principled (tau synthesis)**: the observed lifecycle + `QUESTION:` marker — extends
  "verify, don't trust self-report"; no direct SOTA precedent for the marker.
- **Product (differentiator, out of harness-efficacy scope)**: async / fleet /
  inspectable-in-tab / direct user↔child.
- **Dropped from framing**: the "planner-removal ablation" (unverified; the STEP ablation
  points the other way; not load-bearing).
