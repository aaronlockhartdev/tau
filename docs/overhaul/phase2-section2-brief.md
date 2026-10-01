# Section 2 brief — Core surface: tool routing, child links, turn-end OM (phase 2)

You are the worker for section 2 of the phase 2 refactor — the last section. Read this
brief fully before touching anything. Context: `docs/overhaul/phase2-plan.md` §2; the
R1/R3/R4 candidate cards in `docs/overhaul/architecture-review-20261001.html`;
conventions in `AGENTS.md` (Coding conventions + Best practices + Testing).

**One branch, three items, in this order: R1 → R3 → R4.** All three touch `turn.rs` /
`agent.rs` / dispatch — internal sequence, no parallelism. R3 after R1 (the surface owns
child routing); R4 last (turn loop settled). The session constructor from section 1
(`harness/launch.rs`, `SessionRole`, `AgentSession::launch`) is already on main — build on
it, and **reuse `SessionRole`** wherever a role is needed; do not invent a parallel enum.

## Worktree (mandatory)

```
git -C /Users/aaron/git/tau worktree add /tmp/tau-p2-s2 -b phase2/section2-core-surface main
```

Do all work in `/tmp/tau-p2-s2`. Never edit files under `/Users/aaron/git/tau` directly.

## File ownership (hard boundary)

You may modify: `crates/tau-core/**` (tools/, agent/turn*, subagent/, om_integration/,
harness/ where dispatch is involved). You may NOT modify: `app/**`, `crates/tau-protocol/**`,
`crates/tau-mock-llm/**`, `crates/tau-acceptance/**`, CI files, docs.
Need any of them → stop and report.

## The design (plan §2)

### 1. R1 — tool surface module

Role→tool routing is currently name-string matching smeared across ≥5 places (a documented
past bug lives in the scatter — the load-bearing comment at `turn.rs:318`). One module owns
the whole tool surface:

```rust
specs_for(role) -> Vec<ToolSpec>          // the 7 spec builders, role-filtered
dispatch(role, name, args, ctx) -> ToolOutcome   // the routing table
```

- The routing table covers: core tools, supervisor tools, child link, task store, scoped
  recall — whatever the turn loop matches today (enumerate it from the current code).
- The spawn-time agent-type filter and the child refusal rule move into the surface.
- The turn loop dispatches against its surface and **stops matching tool names**. The
  `turn.rs:318` comment gets a home (the surface module, as the invariant it describes).

### 2. R3 — ChildLink owns the pointer model

The ADR-0001 pointer-model invariant (worker's pane is a projection of the creator's
record) is currently re-derived at 6 harness sites, one of which **parses the handle
string** (`rsplit_once`) against a format minted in `spawn.rs` — a parent id containing
`-` breaks silently.

- `ChildLink` owns: `task_view()` (folded, filtered to the worker), `send`/`stop` routed
  through the parent.
- The handle becomes **opaque**: no string parsing anywhere. `spawn.rs` mints, `ChildLink`
  stores/compares, the dispatch sites call link methods.
- The snapshot builder and the four dispatch sites stop re-opening the parent store.

### 3. R4 — turn-end OM pass as one call

The ADR-0004 turn-end policy sequence (unobserved → activation → plan → observe/reflect)
is split between the turn loop's sequencing and `turn_end.rs` plan/commit — the home of
the 80 GB runaway (now guarded, but the policy is only testable by running a whole turn).

- `OmState::settle_turn(store, …)` — one method owns the sequence, with its own store
  reads and status hook. The turn loop makes **one call**.
- The 80 GB scenario becomes a unit test (cyclic parent chain: guard triggers, terminates,
  bounded output).

## Rules

- **No behaviour change.** Observable behaviour is identical; this is ownership
  consolidation. The acceptance suites (especially live-tools, live-subagent, live-om)
  are your behavioural proof — run them after every item.
- New code obeys the active lints (workspace clippy pedantic cherry-picks, msrv 1.98.1)
  and the AGENTS.md Best-practices rules (conversion naming, bare getters, Display style,
  common traits on public-surface types).
- AGENTS.md comment discipline; 500-LOC soft limit; tests are code (positive + negative;
  negatives assert the specific `Err`).
- Commit per item: `R1 tool surface`, `R3 ChildLink`, `R4 settle_turn` (tests in each).
  Run `cargo nextest run -p tau-core` after every commit.

## New tests (the point of the section)

- The **role→specs table**: per role, the exact spec set (positive per role; negative: a
  role never sees another role's tools).
- **Child refusal** through the surface (a child dispatching a supervisor tool → the
  specific refusal, not a silent no-op).
- **Handle round-trip**: mint → store → route, with parent ids containing `-` (the
  historical breakage case) — no parsing, no ambiguity.
- **Settle sequence**: the turn-end policy driven directly against an `OmState` + store —
  activation threshold, plan shape, observe/reflect commit — no turn loop.

## Verification (all must pass before your final message)

In `/tmp/tau-p2-s2`:
1. `cargo nextest run --workspace`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo fmt --check`
4. `TAU_E2E_MODE=replay just acceptance` — 6/6 including e2e 20/20
   (non-replay acceptance fails on the e2e stress leg — pre-existing issue #39, not yours)

## Final message

Per item: what moved where; the surface/ChildLink/settle signatures as landed; which
routing sites disappeared (count before/after); the `turn.rs:318` comment's new home;
any existing test that had to change and why; new test list; each verification command
with pass/fail; follow-ups you recommend (do not implement).
