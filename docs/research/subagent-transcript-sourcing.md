# subagent_transcript: a window onto the child's working context

> **Status (2026-10-19):** the *model-tool* framing was closed (#193) after SOTA
> review — no mainstream harness gives the parent *model* a live view of a
> child; visibility is a user-side/UI concern. This research survives for the
> refiled GUI child-activity command: its core finding (the assembly seam is
> pure and callable on another session's store via a throwaway `OmState`) is
> exactly what that command builds on.
## 1. The existing context-assembly seam: reusable

The turn's context is built in `agent/turn.rs` under one lock, from two
pieces that are both **pure over the record** (no store access, no running
turn state):

- `OmState::assemble_context(base)` (`om_integration/turn_end.rs:298`) —
  takes the base system prompt, returns `AssembledContext { instructions,
  om_input_items }`: the observation groups as `role: system` items
  (`agent_observations()` = frozen prefix + active suffix) plus the one-shot
  continuation hint.
- `OmState::raw_window_from(entries, leaf_id)` (`turn_end.rs:346`) —
  `branch_entries` + cursor position lookup = the un-observed raw tail, with
  a leading tool run cut so no result is orphaned from its call.

Entanglement check: the only non-pure behavior is the **one-shot flips** in
`assemble_context` — it consumes `self.changed` and
`self.record.om_suggested_response` (mutates the state). Running that on a
**fresh** `OmState::from_config(&om, record)` built from the child's
`OmState::load_record(&mut child_store)` has zero effect on the child's own
persistent state — the flips land on a throwaway. So: **yes, the seam is
callable on another session's store.** The one rule: never call it on the
child's live `OmState`, or the parent's read consumes the child's
continuation hint / suggested-response one-shots.

The child's base system prompt: the tool can pass an empty base (the
observations framing and group items are independent of it) — the design
strips the system prompt anyway.

Reasoning blocks: nothing to strip. Reasoning is request-scoped
(`ResponseRequest::with_reasoning`, `turn.rs`); assistant entries persisted in
the session file carry text/tool output only. The tool just needs to drop the
`instructions` string and, from the raw window, truncate tool outputs.

## 2. Tool shape (reuse the seam)

Under the "window onto the child's working context" framing, the pipeline is:

1. Open the child's store (`SessionStore::for_workspace(&cwd,
   &child.session_id)` — the proven `state_info` pattern,
   `subagent/inspect.rs`).
2. `record = OmState::load_record(&mut store)` (degrades to empty record on
   corrupt payload — a young child is a legitimate empty log, not an error).
3. Read entries + leaf **once**, before the record read (skew direction, §3).
4. Assemble on the throwaway state: `assemble_context("")` +
   `raw_window_from(&entries, leaf)` → the child's working context.
5. Strip: drop `instructions`; project the raw window compact (role, tool
   name, status, truncated tool outputs); tail-cap both parts.

Hand-rolling the two-section projection is the fallback only if step 4 proves
impossible — it isn't: both functions are pure. Reusing keeps the tool
byte-faithful to what the child's model actually sees (group boundaries,
window cuts, prefix/active split), so "what is the child seeing" and "what
did we send it" can never drift.

## 3. Mid-turn consistency: safe, with two guards

Record read and entry-list read are separate file reads while the child
appends. Failure modes:

- **Tail slightly too long** (child commits an observation between reads):
  the tail re-includes entries the log already summarized. Harmless — the
  parent sees a fact twice in a snapshot.
- **Cursor miss → whole branch.** If the `om` entry is read *after* the entry
  list, the cursor is newer than the list: the position lookup misses and
  `raw_window_from` falls back to the **entire branch** — a 10k-entry child's
  full transcript, the exact blowup #193 exists to prevent. Same trap with a
  corrupt newest-`om` entry (empty record → no cursor → whole branch). Guard:
  the tool's tail cap must apply **unconditionally**, including on cursor
  miss (and report a `stale`/`degraded` flag). Reading entries first, record
  second, biases the skew toward the harmless direction.

Verdict: acceptable — a supervisory snapshot, not a consistency boundary.

## 4. Typical live-tail size at defaults

Defaults (`config.rs` `Om::default`): `observe_threshold: 30_000`,
`buffer_increment`/`buffer_tokens` 6,000; `pending_tokens` is a soft
estimate, `chars/4` (`om.rs::token_count`). In steady state the live tail is
**~30k tokens (~120 KB raw)** — *not* small. It is bounded per **turn**
(`settle_turn` recomputes and activates at turn end), so a single long turn
pushes it past; and up to one buffer cycle (~6k) may be observed but not yet
in the record. The tool's own caps (tail N entries + per-entry truncation)
are therefore load-bearing, independent of the cursor.

## 5. Observation-group shape

Parsed groups, not raw text — `om::parse_observation_groups`
(`om/groups.rs:39`) → `ObservationGroup { id, range, kind, content }`. The
id (16-hex) is the child-`recall` handle (drill-down), `range` is
provenance, `kind = "reflection"` marks Reflector rewrites. Example from
`om/tests_groups.rs`:

```
<observation-group id="0123456789abcdef" range="00000001:00000012">
* 🔴 (14:30) User prefers direct answers
</observation-group>
```

Bullet lines, `(HH:MM)` prefixes, one fact per line; a group from a 6k-token
buffer chunk is a few KB. Expose the last N groups, truncated.

## Recommendation

**Reuse the assembly seam** (`assemble_context` + `raw_window_from` on a
throwaway `OmState` over the child's store), then strip system prompt,
truncate tool outputs, tail-cap. This is strictly better than a
hand-rolled projection: it reproduces the child's actual working context, so
the tool's output is by construction "what the child sees".

Gotchas, in priority order: (1) never touch the child's live `OmState` —
one-shot flag consumption; (2) unconditional tail cap, including on
cursor-miss / empty-record fallbacks; (3) read entries before the record;
(4) young children have an empty observation section — the raw window alone
is the answer, by design, not a failure.

Naming: the issue's `subagent_transcript` undersells it — the first section
is not a transcript at all, it's distilled memory. The tool is a window onto
the child's **working context**; a name like `subagent_context` (or
`subagent_context_window`) states what it returns and why it's bounded.
`subagent_transcript` is fine to keep if it is already referenced in
roadmaps; the rename is cheap before implementation since nothing ships
yet.
