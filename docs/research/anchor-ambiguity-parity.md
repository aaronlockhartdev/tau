# Ambiguous/duplicate anchors in `read`/`write`/`edit` — parity with `pi-hashline-edit-pro`

**Question.** Where do ambiguous (one anchor → several lines) or duplicate (one anchor in
several files) anchors actually occur in tau's `read`/`write`/`edit` tools, how does the
reference implementation (`pi-hashline-edit-pro`, npm `pi-hashline-edit-pro`; investigated at
HEAD 6.5.1, commit `9b049d1` — the EEB top scorer; `docs/research/hashline-pro-diff.md`
cited 6.4.8) behave in the same situations, and what does logical parity require?

**Sources.** Tau (worktree `anchor-ambiguity-parity`): `crates/tau-core/src/hashline/mod.rs`
(`resolve_anchor`, `EditError`, `plan_edits`, `lcs_reanchor`), `crates/tau-core/src/hashline/
registry.rs` (`allocate`, `mint`, `resolve_offset`, `edit_target_error`,
`owners_differing_only_by_case`), `crates/tau-core/src/hashline/pin_store/mod.rs`,
`crates/tau-core/src/tools/impls.rs` (`read`/`write`/`edit`), `crates/tau-core/src/tools/
edit_diff.rs`. Pro files cited: `src/hashline/resolve.ts`, `src/hashline/hash.ts`,
`src/anchor-registry.ts`, `src/read.ts`, `src/edit-common.ts`, `src/batch.ts`.

## 1. The failure modes

Anchor uniqueness is an *allocation invariant* (per-file bitset + probing, session-wide
`owned` map), not a property of the hash function — so in normal operation an anchor is
unique by construction. The ambiguous/duplicate surface is what happens **when the
invariant is broken, or when the model's reference is ambiguous**:

1. **Duplicate in one file (broken set).** A pin-store record written by an older buggy
   version, a hand-edited store, or a torn write can pin one anchor to two lines.
   `registry::allocate` seeds a re-read from the store *and reuses the pinned anchors for
   matched lines without any cross-check*, so the duplicate propagates into the `read`
   output and into every later edit's anchor set (`lcs_reanchor` keeps it;
   `refresh_owned` re-persists it).
2. **Cross-file duplicate (stale seed).** A pin store is per-workspace; nothing stops
   file B's record from pinning an anchor that this session already owns for file A.
   `allocate(B)` would reuse it, and the single-entry `owned` map would then point the
   anchor at B while A's `read` output still shows it on A — the "owned by …"
   ambiguity, with A's edits silently re-attributed.
3. **Ambiguous model reference (case-mangled anchor).** The model upper/lower-cases an
   anchor. The string is unowned, and both implementations answered with a bare
   "not owned / stale — re-read", sending the model to re-read a file whose anchors are
   fine (a wasted turn, and the model often re-mangles).
4. **Ambiguous edit target.** A `from`/`to` pair whose bounds are owned by *different*
   files, or owned by a file other than the one the edit names.

## 2. The reference's behavior (pro)

- **Duplicate in one file → first occurrence, deterministically.** `valEdit` builds
  `hashIndex: Map<anchor, number[]>` and `resAnchorFromMap` returns
  `hashMatches[0]` (`src/hashline/resolve.ts`). There is **no "ambiguous" concept at
  all** — a broken set is simply resolved to its first line, and the edit proceeds.
- **Read with anchor offset.** `resolveReadOffset` (`src/read.ts:200`): ownership check
  first — unowned → `[E_STALE_ANCHOR] "X" is not owned in this session.` **plus a
  case-folding hint** (`ownersDifferingOnlyByCase`, `src/anchor-registry.ts:717`):
  `Anchors are case-sensitive; "Y" differs only in case.`; owned by another file →
  `owned by {path}`; else first-occurrence resolution, with the diagnostic's fresh
  anchors adopted before the re-throw.
- **Edit target gate.** `resolveEditTarget` (`src/edit-common.ts:211`), run *before*
  planning: any bound unowned → `[E_STALE_ANCHOR] "X" is not owned in this session.{hint}
  Call read() on the target file first.` (the hint is scoped to the paths the *other*
  bounds own — and an empty scope means *no filter*, `paths.size > 0`); bounds owned by
  two distinct files → `[E_BAD_SHAPE] The anchors are owned by different files (A, B);
  edit one file per call.`
- **Cross-file collision at allocation.** `alignOwnershipWithSpans`
  (`src/anchor-registry.ts:817`): a positional anchor is reused **only if**
  `!state.owned.has(anchor) || state.owned.get(anchor).path === path` — a pin owned by
  another file is never reused; a fresh anchor is minted instead.
- **Duplicate anchors in output are never deduped.** Read renders the store's per-file
  hash set as-is; a broken set shows both rows. Uniqueness is restored by the
  first-occurrence rule, not by a dedupe pass.

## 3. What tau did before, and the parity changes

| # | Situation | tau (before) | pro | change on this branch |
|---|---|---|---|---|
| 1 | Edit bound duplicated in the file | `EditError::Ambiguous` — whole batch rejected (or the member demoted) | first occurrence | `resolve_anchor` resolves to the **first occurrence**; `EditError::Ambiguous` removed (its only construction site) |
| 2 | Cross-file duplicate via store seed | reused → `owned` map flip-flops, "owned by" ambiguity | re-minted (the `alignOwnershipWithSpans` guard) | `registry::allocate` re-mints a pinned anchor the session owns for a **different** file; same-file pins still reuse |
| 3 | Case-mangled anchor, read offset | bare "not owned" | + case-folding hint | `registry::resolve_offset` gains the hint via new `owners_differing_only_by_case` (sorted; empty path-scope = no filter, matching pro) |
| 4 | Case-mangled / unowned anchor, edit | fell through to `Stale` ("re-read the file") | pre-plan gate with the reference's messages | new **ownership gate** in `edit()`: per-member `registry::edit_target_error` → typed `EditError::{NotOwned, CrossFile, OwnedElsewhere}` (E-codes `[E_STALE_ANCHOR]` / `[E_BAD_SHAPE]`). Strict mode: the batch rejects. Demotion mode (the default): the offending members are reported on the existing `[H_DEMOTED]` channel and the rest apply — the batch planner's per-member demotion, parity with `src/batch.ts` |
| 5 | Broken set after an edit | duplicate propagated into the new set | unchanged lines keep their (duplicated) anchors; the set is re-anchored over the used bitset | no change needed — `lcs_reanchor` already mints around the used set, so the duplicate does not survive an edit (asserted by test) |

All edit failures — including the new gate — surface as the **typed** `EditError`
through the existing `format!("edit: {e}")` / `[H_DEMOTED] edits[{i}] not applied: {e}`
paths; no ad-hoc strings cross the tool boundary.

## 4. Known remaining divergences (deliberately out of scope)

- **Read-before-edit gate.** Pro's ownership check runs for *every* edit, so a file the
  session never read is refused ("not owned … Call read() on the target file first.").
  Tau deliberately allows a content-addressed edit of an unknown file (ADR-0033: a fresh
  session re-derives the same anchors). The gate therefore applies **only when the file
  is known to the registry** (`anchors_for` is `Some`); unknown files keep today's
  resolution.
- **Resolution-level stale message shape.** Pro's `[E_STALE_ANCHOR] … The file changed
  since read.` with ±1 context rows (`fmtMismatchWithHashes`) vs tau's current
  `Stale` display. That is the broader rich-diagnostics port (dimension 17 in
  `docs/research/hashline-pro-diff.md`), not an ambiguity behavior.
- **Served map scope.** Pro's `served` is session-global (pruned per file at
  `markServed`); tau's is per-file. With the allocation invariants (and change #2 above)
  anchors are session-unique, so the two behave the same on every reachable state.
- **Edit re-anchoring.** Pro uses a true LCS (`Diff.diffArrays`, dimension 7); tau's
  `lcs_reanchor` is a single-pass checksum walk. A changed line before a survivor can
  cost the survivor its anchor on the edit path (re-reads use the true LCS via
  `registry::match_anchors`). Unrelated to ambiguity; ported when dimension 7 lands.
- **`swapReversedRanges` last-wins.** Pro's internal reversed-swap helper maps an
  anchor to its *last* line (`lineByHash.set` overwrites); the canonical resolution is
  first-occurrence, which is what tau now matches.

## 5. Tests

- `crates/tau-core/src/hashline/tests.rs` — `duplicate_anchor_resolves_to_first_occurrence`
  (positive: the edit lands on line 1; the set is unique again),
  `duplicate_anchor_at_a_later_line_is_unreachable_by_reference` (positive: the range
  starts at the first occurrence; the second is untouched),
  `duplicate_set_still_rejects_unknown_anchors` (negative: `Stale` is unchanged),
  `ownership_gates_render_their_e_codes` (the three typed variants' display, incl. the
  case hint and the plural/singular forms).
- `crates/tau-core/src/hashline/registry.rs` — `store_seeded_pin_owned_by_another_file_is_reminted`
  (negative: no cross-file duplicate) and `store_seeded_pin_owned_by_the_same_file_is_reused`
  (positive: the normal re-read case is not clobbered),
  `resolve_offset_hints_on_case_folded_anchors` (positive hint / negative no-hint),
  `edit_target_error_is_typed_per_failure_mode` (all five gate outcomes, incl.
  `None` for a registry-unknown file).
- `crates/tau-core/src/tools/tests.rs` — end-to-end over the tool surface:
  `edit_with_unowned_anchor_is_a_typed_ownership_error`,
  `edit_unowned_anchor_case_hint_names_the_owned_variant`,
  `edit_cross_file_anchors_are_a_bad_shape` (strict: whole batch rejects, file
  untouched), `edit_batch_demotes_unowned_members_and_applies_the_rest` (demotion:
  valid member applies, the gated one reports its typed error).
