# Subagent cleanup policy: bounding the child fleet

> **Status (2026-10-20):** design pass for #230. The recommendation (§8)
> replaces the issue's working hypothesis (2+3 with 1 as backstop) with
> **spawn-triggered terminal reclamation + a total-handle cap +
> re-adopt-on-address** — a deterministic variant of option 3, with option 2
> rejected outright and option 4 deferred.

## 1. The fleet is three registries, and only one is the problem

A spawned child exists in three in-memory places, with three different
lifetimes:

| Registry | Owner | Content | Today's lifetime |
|---|---|---|---|
| `Supervisor.children` (`subagent.rs:53`) | the parent's `AgentSession` | `Arc<Child>` per handle — the child's state, its own `AgentSession`, its drive | "until the parent session closes" — the map's doc comment (`subagent.rs:48`) says so explicitly: *bounded by the spawn count for the session's life* |
| `Core.child_handles` (`harness/state.rs:108`) | the core | handle → parent, seeded from `spawned` bridge events (`forwarding.rs:316`), **never from disk** | process lifetime, no removal path |
| `Core.sessions` (`harness/state.rs:49`) | the core | the child's `LiveSession` — the child is "an ordinary session" (ADR-0006), so it stays registered until the user closes it | user-driven (`SessionClose`) |

The rows are **not** a registry. A child's row — its session file, its tree
row in the GUI — is disk state: the file header's `parent` field plus the
last `subagent` state entry. Both the sub-agents tab's disk-restore
synthesis (`openSession`, `app/svelte/lib/sessions.ts:244–259`) and the
snapshot's restart fallback (`restore_children_from_disk`,
`subagent/restore.rs`) reconstruct rows from disk alone. Rows are already
bounded by what the user archives and by the session store's own lifetime.

So the #230 problem is exactly one thing: **the in-memory handle fleet grows
monotonically with spawn count and has no removal path.** Each retained
`Arc<Child>` keeps the child's whole `AgentSession` alive — provider, OM
state, prompt — long after the child finished. The `subagents` projection
in snapshots (`harness/snapshot.rs:101–108`) then grows with it.

## 2. The unit of cleanup: in-memory handle only

Reclamation removes the entry from `Supervisor.children` (and the matching
`Core.child_handle` bookkeeping). It **never** touches the session file, and
it **never** removes the child from `Core.sessions` — a child the user has
open in the GUI must keep working.

This answers the issue's first question directly: conflating the handle's
lifetime with the row's is the current bug. The design keeps them separate:

- **Handle** (reclaimable): the supervisor's ability to route to and resume
  a specific `Arc<Child>` without disk work.
- **Row** (permanent until archived): the child's identity and history on
  disk, which the GUI and restart-restore read directly.

A reclamation that also hid the row would destroy the restart-restore
ground truth; one that left the row visible but made the child unaddressable
would create zombies (§5). Handle-only is the only unit that is safe on its
own *if* the re-adopt path exists — which is part of the recommendation,
not an option.

## 3. Evaluating the five options

**Option 1 — cap at spawn.** Today's `check_cap` (`subagent.rs:269`) counts
only **live** children (`live_children` filters out done/stopped/failed), so
a terminal child costs zero against `max_concurrent`. A spawn cap as
currently defined **does not bound the fleet at all** — the fleet is exactly
the terminal children, and those never count. Option 1 is only meaningful
rephrased as a *total-handle* cap on the map, which is the backstop in the
recommendation (§6) rather than the primary mechanism: as a primary it
either refuses legitimate spawns (harsh, and it forces the model to
handle a new failure mode for a memory-management detail) or it evicts,
which is really option 3 wearing a hat.

**Option 2 — name/handle reuse. Rejected.** Handles are durable identities:
`{parent_session}-{counter}`, with the counter seeded from the parent's
durable spawn records so a re-opened session never re-mints a handle an
earlier life used (the #226 fix; `subagent.rs:160`, `minted_count`).
Reusing a reclaimed handle would silently re-attach a *new* child to an
*old* child's session file, routing, and history — the exact conflation the
#226 fix exists to prevent. It also breaks the GUI's row identity (rows key
by handle and child session id). Names are not handles: a name collision
already fails today, and name reuse is a naming policy, not a cleanup
policy.

**Option 3 — TTL reclamation sweep. Adopted, with two changes.**
(a) *Event-triggered, not timed.* The fleet grows one handle per spawn, so a
bound checked **at spawn** — evicting the oldest terminal children until
back under the bound — is a deterministic, testable subsumption of a
background TTL timer. No timer task, no "how stale is stale" knob, no
races between sweep and drive. (b) *Terminal-only.* The sweep may only
reclaim `done`/`failed`/`stopped` children. `idle`/`awaiting` are parked,
not dead: the child is resting on a declared `waiting_on` (possibly a
queued message, possibly a `QUESTION:` awaiting the user), and #187/#188
make "a message resumes it" a contract the parent model relies on.
Reclaiming a parked child converts that contract into a zombie (§5).
A `parent_notify` in flight is the same category — it is a pending
interaction, and its result lands in the session file, which re-adopt
reads.

**Option 4 — parent-explicit `subagent_reap`. Deferred.** The ticket's own
doubt is correct: it depends on the model remembering to call a
memory-management tool, which it won't, consistently. It also adds IPC
attack-surface for a concern the core should own. Revisit only if the
automatic policy proves surprising in practice (e.g. the model repeatedly
re-addresses a reclaimed child and pays the re-adopt cost).

**Option 5 — GUI-side archive. Orthogonal.** The archive flag already
exists and already filters `restore_children_from_disk` (its `!m.archived`
predicate). It bounds *row visibility*, not *memory*, so it is a user
preference on top of the policy, not part of it. No change required by this
design; it is the user's lever if they want the historical rows gone.

## 4. The zombie problem (why re-adopt is mandatory)

After a handle-only reclamation, what happens to each address path:

- `Supervisor::resolve` (`subagent.rs:287`) searches **only** the in-memory
  map → miss → `subagent_message`/`subagent_stop`/`subagent_state` all fail
  with "no sub-agent in this session".
- The GUI routes the same way: `child_parent(handle)` → `sup.message(...)`
  (`dispatch_commands.rs:162, 215, 242, 263`).
- But the child is *visible*: the row survives on disk, the tab shows it
  (disk-restore synthesis), and the model can read its transcript.

A child the parent can see and must talk to, but can't reach, is a zombie —
worse than no reclamation at all, because the model now gets a confusing
failure for a child that is plainly in its own session. Reclamation is
therefore only safe with a **re-adopt** path: on a `resolve` miss, fall
back to the disk records that `restore_children_from_disk` already parses —
the session file whose header names this parent, matched by name / session
id / handle, and whose last recorded state is terminal. Re-adopt
reconstructs the `Child` around a freshly opened `AgentSession` for that
file (the spawn flow already handles an existing session file; the
spawn record in the file carries the agent type and context mode, and the
last state entry the terminal state) and re-inserts it into the map and
`child_handles`. The cost is one disk read + session open, paid only when
the parent model (or the GUI) actually addresses the reclaimed child —
which is rare by construction, since the fleet bound keeps the hot set
in-memory.

With re-adopt, reclamation is **transparent to the parent model**: the
contract "you can message any child of this session, and a message resumes
it" (§187/#188) is unchanged. Reclaimed simply means "cold".

## 5. What may be reclaimed — summary

| State | Reclaimable? | Why |
|---|---|---|
| `done` / `failed` / `stopped` | yes | no in-flight interaction; every open record (state, `parent_notify` output, queued-message resolution) is already in the session file, which re-adopt reads |
| `idle` / `awaiting` | **no** | parked on a live interaction (declared `waiting_on`, queued message, user question); reclaiming breaks the #187/#188 resume contract |
| `running` | **no** | it is live; it holds a concurrency slot by definition |

## 6. Recommended policy

1. **Unit:** in-memory handle only (`Supervisor.children` entry + the
   `Core.child_handles` bookkeeping). Session file and row visibility are
   never touched.
2. **Trigger:** at spawn, not on a timer. After a successful spawn, if the
   map's total handle count exceeds a soft bound, evict the **oldest
   terminal** children (terminal-state, lowest counter in the handle — the
   counter is the mint order) until back under the bound.
3. **Backstop:** a hard cap on *total* handles (the rephrased option 1).
   The soft bound evicts; the hard cap is the invariant the soft bound
   exists to maintain, so it is a defensive assertion path, not a
   user-facing failure.
4. **Re-adopt:** resolve-miss falls back to disk (§4) for terminal children
   only; a reclaimed `idle`/`awaiting` child is impossible by rule 2's
   evictor, so no special case is needed there.
5. **Config:** two new fields on `SubAgents` (`config.rs:246`), e.g.
   `fleet_soft_bound` and `fleet_hard_cap`, both `Option<u32>`, defaulting
   to values well above any realistic eval load (see §7).
6. **No GUI change required.** Reclaiming a handle does not remove its row:
   the snapshot's `live_subagents` projects the remaining handles and the
   GUI's synthesis tops the list up from disk (`sessions.ts:244–259`), so
   the reclaimed child's row persists — as its last recorded state, which
   is exactly the historical entry the ticket wants. (The snapshot's
   all-or-nothing registry/disk fallback at `snapshot.rs:101–108` should
   become a *union*: registry handles projected live, plus any
   disk-children not in the registry projected from disk — otherwise a
   partially-reclaimed fleet makes the early children disappear from the
   snapshot until restart. This is a one-function fix in scope for the
   implementation ticket.)

## 7. EEB / eval impact

The mock fixtures are small: `e2e-workbench-parent.json` spawns 2 (the
maximum); `acceptance-subagent-parent.json`, `accept-sender-labels-parent.
json`, and `e2e-subagent-parent.json` spawn 1. A soft bound of 32 is 16×
the largest scenario and 16× today's default `max_depth`-1 realistic
fan-out; no scenario can touch it. The stress leg (10k-entry fixture)
stresses the *transcript* path, not spawn count. EEB needs no changes; a
new acceptance scenario that spawns past the bound and re-addresses an
evicted child is the one worth adding (it tests re-adopt end-to-end).

## Recommendation

**Spawn-triggered terminal reclamation under a total-handle bound, with
re-adopt-on-address making it transparent.** The issue's hypothesis is
right in shape (bound the fleet, backstop it) and wrong in two details:
option 2 is not just weak, it is *incompatible* with the #226 durable-handle
fix, and a TTL timer buys nothing over the spawn-triggered bound — it adds
a background task and a time knob to solve a count problem. Option 1's
`check_cap` counts live children only, so it bounds concurrency, not the
fleet; the fleet cap is a separate, total-count quantity. The load-bearing
new piece the hypothesis is missing is re-adopt: without it, *any*
reclamation policy produces zombies, and with it, reclamation is invisible
to the parent model.

Follow-up implementation ticket: #250.
