# Audit: hashline surface parity with pi-hashline-edit-pro

Issue: full-surface follow-up to PR #285 (ambiguous/duplicate anchor parity) · Audited 2026-10-23

**Scope.** The anchor `read`/`edit` tools and the anchor **registry** (allocation,
serving, LRU reclaim, persistence). Pro's other tool families (`replace`,
`replace_match`, `insert`, `copy`/`move`, `grep`, `undo_last_change`, …) are
**out of scope by decision** — we are not implementing them; they are a
toolset difference, not a hashline-behavior gap. Within scope, "parity" means
the model sees equivalent behavior for the same request; wording differences
that carry the same information count as parity.

**Baseline.** Tau at `main` (d339ff6) plus in-flight PR #285 (which closes the
four ambiguity/ownership dimensions audited in
`docs/research/anchor-ambiguity-parity.md`). Pro at v6.5.1 (commit 9b049d1,
cloned at the audit worktree's `/tmp/hashline-pro`).

**Verdicts.** `at parity` = equivalent behavior · `intentional` = designed
difference (ADR/ticket), documented, not a gap · `gap` = pro has it, tau
lacks it, port candidate (§3).

## 1. Read

| # | Behavior | Pro (citation) | Tau (citation) | Verdict |
|---|----------|----------------|----------------|---------|
| R1 | Row rendering `HASH│content` | `resolve.ts:73` `fmtRow` | `impls.rs:289` `render_page` | at parity |
| R2 | `offset` as line number **or** served anchor; `limit` counts from the anchor row | `read.ts:318` schema, `resolveReadOffset` `read.ts:200` | `impls.rs:145-160` (digits → line; else `reg.resolve_offset`) | at parity |
| R3 | Beyond-EOF offset message | `read.ts:92` | `impls.rs:168` — **verbatim identical** | at parity |
| R4 | Empty file | `read.ts:66` `HASH│` + `[File is empty. Use replace to insert content.]` | anchored empty row, no note | intentional — pro's note names pro's `replace` tool; both expose an insertable anchored line |
| R5 | Truncation + pagination hint offering the last **served anchor** as next offset | `read.ts:47` `formatPaginationHint` | `impls.rs:210` `… truncated (showing lines X–Y of Z — N bytes). Use offset=A to continue.` | at parity (wording) |
| R6 | Oversized lines: hidden, **not served**, listed in a warning | `read.ts:118-152` per-line inline `sed -n 'Np'` hint + summary (cap 10) | `impls.rs:240-252` summary `[N oversized line(s) over 100 KiB not shown: lines …]` (cap 10) | gap (minor, G6) — pro's per-line `sed` hint is more actionable |
| R7 | Non-UTF-8 bytes | `read.ts:383` notice: shown as U+FFFD, edit rewrites as UTF-8 | `impls.rs:97` BOM/UTF-16 decoded; BOM-less non-UTF-8 **refused** | intentional — #207 design |
| R8 | Image reads | `read.ts:352` delegates to the builtin read tool | `impls.rs:47-110` own JPEG-q85 pipeline (byte cap, pixel-budget downscale, #36/#252/#257) | intentional — tau design |
| R9 | Anchor-reclaim notice appended to the next read: `[W_ANCHOR_RECLAIMED] … freed all anchors of {list}, the files least recently read or edited. Read each freed file again before editing it.` | `anchor-registry.ts:604-611` (adds pro-only `/clear-anchors` tail) | `registry.rs:382-405` (same sentence, cap 10) | at parity |
| R10 | Only **shown** rows are served (integrity baseline) | `read.ts:378` `serveRows` | `impls.rs:222` `reg.serve(path, &page.served)` | at parity |
| R11 | Anchor-offset ownership gate: parse-first, unowned → `E_STALE_ANCHOR` + case hint, cross-path → "owned by …" | `read.ts:208-224` | PR #285 (`registry.rs resolve_offset`) | at parity (#285) |

## 2. Edit

| # | Behavior | Pro (citation) | Tau (citation) | Verdict |
|---|----------|----------------|----------------|---------|
| E1 | Malformed-ref diagnostics: four shape-specific `[E_BAD_REF]` messages (empty / line-number / multi-line `HASH│` block / `│`-suffixed) with concrete repair text | `parse.ts:7-35` `diagRef` | single generic `E_BAD_ANCHOR` "expected a bare 4-letter alphabetic anchor" (`mod.rs`) | **gap (G1)** |
| E2 | Duplicate in-file anchor → first occurrence | `resolve.ts:47-59` | PR #285 (`mod.rs resolve_anchor`) | at parity (#285) |
| E3 | Pre-plan ownership gate: verbatim messages, parse-first, unowned shadows cross-file | `edit-common.ts:222-239` | PR #285 (`impls.rs` + `registry.rs edit_target_error`) | at parity (#285) |
| E4 | Target file: pro **infers** it from anchor ownership; `path` optional, must match when given (`requirePath` flag) | `edit-common.ts:241-290` | `path` is a required explicit argument | intentional — tau tool contract (spec §5.4) |
| E5 | Lenient fix: reversed range swapped, member continues | `resolve.ts:265-283` `[H_RANGE_REVERSED]` | `mod.rs:481` `[W_RANGE_REVERSED]` | at parity (code prefix differs) |
| E6 | Lenient fix: pasted `HASH│` prefixes stripped | `resolve.ts:257` `[W_BARE_HASH_PREFIX]` | `mod.rs:519` same code | at parity |
| E7 | Lenient fix: pasted diff-preview markers (`+`/`-` rows) stripped | `resolve.ts:261` `[W_INVALID_PATCH]` | none | **gap (G2)** |
| E8 | Strict mode rejects the auto-fixable slips instead of fixing | `edit-common.ts:297` `STRICT_INPUT_REJECT_CODES` = `[W_BAD_SHAPE]`,`[W_BAD_REF]`,`[W_INVALID_PATCH]`,`[W_BARE_HASH_PREFIX]` | `mod.rs:187` `E_ANCHORED_CONTENT`, `mod.rs:481` reversed rejected | at parity (substance; code lists differ) |
| E9 | Stale-anchor diagnostic: ±1-line context **with fresh anchors**, adopted for retry | `resolve.ts:86-117` `fmtMismatchWithHashes`; adoption `read.ts:222`, `batch.ts:737-738` | lean `E_STALE_ANCHOR` naming the other bound's line | **gap (G3)** — the known rich-diagnostics port (parity doc §4) |
| E10 | Range-served integrity: per-line checksums; **deletion interiors exempt** (bookends only) | `resolve.ts:405-452` `assertRangeServed` | `mod.rs:425-444` — both properties present | at parity |
| E11 | `E_RANGE_STALE` shape: count of mismatched lines, display **capped at 100** with "Retry with the fresh anchors above without a read." / read-with-offset continuation | `resolve.ts:430-450` | `mod.rs:199` — whole range, fresh anchors, no cap note | gap (minor, G5) |
| E12 | Per-line line-ending preservation across the edited span | `line-endings.ts:66-120` `separatorsForSpans` | `line_endings.rs` (ported) | at parity |
| E13 | No-op edit → "no change" report (not a silent success) | `apply.ts:60-75` `resToSpan` noop; `batch.ts:898` | `edit_diff.rs:16` `edited {path} (no change)`, guarded since #180/#235 | at parity |
| E14 | Post-edit re-anchoring: unchanged lines keep anchors | true LCS (`Diff.diffArrays`) | one maximal in-order alignment: `hashline/lcs.rs` `carry` (the greedy walk and the registry's DP table are deleted, #298) | at parity (#298) — pro's structural carry is cheaper for app writes: G7 |
| E15 | Post-edit diff: pro's model-facing diff carries anchors on `-`/` `/`+` rows ("rows the diff does not show keep their anchors") | `edit-common.ts:87` guideline; `replace-diff.ts` | model ack is counts; bounded diff (anchors on `+` rows only) rides the GUI detail channel (#243) | intentional — #243 channel split |
| E16 | Literal `\uDDDD` in content → notice, no autocorrection | `resolve.ts:218-226` | none | **gap (G4)** — tiny |
| E17 | Batching: pro batches same-file calls **across one message** (`In batch N (queued)` acks, combined diff, one undo, `[E_OP_ABORTED]` whole-batch abort); tau takes an `edits` **array per call** with per-member demotion | `batch.ts:516,587-595,617-632` | `impls.rs edit()` `edits` array + `[H_DEMOTED]` per member | intentional — different batch models; per-member demotion parity holds within a call (PR #285) |
| E18 | New-content payload: pro `text` is an **array of strings** (one per line); tau `content` is a single string | `parse.ts:60-90` `parseText` | `content: string` | intentional — tau tool contract (spec §5.4) |

## 3. Registry

| # | Behavior | Pro (citation) | Tau (citation) | Verdict |
|---|----------|----------------|----------------|---------|
| RG1 | Minting: probe for a free slot; on exhaustion reuse-then-LRU-reclaim (protecting the current file); final error "pool exhausted; use write for very large files" | `anchor-registry.ts:572-611` `mintAnchor` (8192-probe limit) | `registry.rs:316-339` (file-hint probe continuation, `TooManyLines`) | at parity (substance) |
| RG2 | `everMinted`: an anchor minted once in a session is **never re-minted** (frees don't recycle slots mid-session) | `anchor-registry.ts:199,631` | freed slots recycle immediately; cross-session stability comes from the pin store instead | intentional — design split: pro is session-scoped stateful, tau is content-addressed (ADR-0033); PR #285 closed the resulting cross-file seed-collision class |
| RG3 | LRU reclaim of the coldest path, warning on the next read | `anchor-registry.ts:554-570` | `registry.rs:362-380` | at parity |
| RG4 | Persistence: pro folds a **session log** (allocate/free/serve events, compact sidecar) back into state at session start | `anchor-registry.ts:213-440` | pin store: content-addressed per-file anchors + checksums, cross-session, GC (#225) | intentional — both solve resume-stability; different mechanisms |
| RG5 | Served map: pro session-global (pruned per path); tau per-path | `anchor-registry.ts:685-706` `markServed` | `registry.rs:172` per-path map | documented, equivalent in practice (global uniqueness at mint) |
| RG6 | Diagnostic **adoption**: pro registers fresh anchors from stale/range-stale diagnostics so a retry works without a re-read (stateful) | `batch.ts:737-738`, `read.ts:222` | unnecessary — stateless re-derivation: anchors are content-stable, so the retry re-resolves identically (ADR-0033) | intentional |

## 4. Gaps (port candidates, ranked)

1. **G1 — shape-specific malformed-ref diagnostics** (`[E_BAD_REF]` × 4 shapes,
   `parse.ts:7-35`). High model-behavior value: the multi-line-block message
   tells the model exactly which hash to use as `from`/`to` and to move the
   block's content into `content`. Small, self-contained port into
   `resolve_anchor`'s `BadAnchor` path (the message is the only change).
2. **G3 — rich stale diagnostics** (±1-line context with fresh anchors,
   `resolve.ts:86-117`). The known rich-diagnostics port; larger (needs the
   other-bound context + a served-map for the context rows). tau's adoption
   equivalent is a no-op by ADR-0033.
3. **G2 — diff-preview-marker stripping** (`[W_INVALID_PATCH]`,
   `resolve.ts:261`). Closes the case where the model pastes a diff preview
   (`+ hash│line` rows) as new content; pairs naturally with G1.
4. **G5 — range-stale display cap** (100 rows + continuation hint,
   `resolve.ts:430-450`). Cheap hardening for huge ranges.
5. **G4 — `\uDDDD` notice** (`resolve.ts:218`). One-line port.
6. **G6 — per-line `sed` hint on oversized read rows** (`read.ts:118-152`).
   Cosmetic actionability; tau's line listing already names the lines.

None of the six blocks the merge of PR #285; each is an independent follow-up
ticket. The rest of the surface is at parity or an intentional, documented
design difference.


**Resolution (this branch).** All six are now closed:

- G1, G2, G3, G4, G5 live in `hashline/diagnostics.rs` (shared helpers) wired
  into `EditError::BadAnchor` (shape-specific `E_BAD_REF`),
  `EditError::InvalidPatch` + `PlanOptions.path`, `EditError::Stale` carrying a
  `StaleContext` (±1-line block with fresh anchors), the `lenient_fixes`
  warnings, and the `E_RANGE_STALE` 100-row cap with the continuation hint.
  `lenient_fixes` no longer pre-empts the resolution phase on unresolvable
  bounds, so the rich stale diagnostic is the standard stale path.
  One known residual vs the reference: when the context neighbourhood was
  never served, a first retry with the context anchors bounces through one
  `E_RANGE_STALE` (which adopts them) before succeeding — the reference's
  `adoptAnchors` avoids the bounce; a consequence of the ADR-0033 split,
  not a defect.
- G6 lives in `tools/read_page.rs` (the read-page rendering extracted out of
  `impls.rs` for the 1000-LOC gate): oversized lines render an inline
  `sed -n 'Np'` placeholder and stay served, so a targeted edit by their
  anchor works.

Two deliberate adaptations vs the reference: the diff-marker detector also
accepts tau's own diff-preview shape (`+ AbCd│…` with a space after the sign),
and tau keeps its letters-only anchor alphabet (the block-shape `E_BAD_REF`
names only parseable anchors). Tests: `hashline/tests_diag.rs` (G1–G5) and
`tools/tests_read.rs` (G6).

**E14 re-opened and closed (#298, 2026-10-27).** This audit accepted the greedy
walk as `intentional/known` on the theory that a shorter common subsequence
costs only churn. Measured, the theory fails: the greedy walk and the
registry's dynamic program disagree on 57,743 of 200,000 random line pairs, so
the same content re-anchored differently depending on which path produced the
read — an anchor served by an app-write read had a smaller chance of surviving
the next edit than one served by a registry re-read. One primitive now answers
all three callers: `hashline/lcs.rs::carry`, maximal, `O(n·m)` time and
`O(min(n, m))` space (Hirschberg's divide and conquer). The greedy walk and the
registry's full table are deleted; the table survives as the maximality oracle
in `lcs.rs`'s tests and as `fresh_lines_by_full_table` in `properties.rs`, and
`a_re_read_relabels_no_more_lines_than_the_maximal_match` pins the §7.3 bar in
the anchors a caller receives. That also settles the memory complaint: no
production path builds an `n × m` table any more.

- **G7 — structural carry for app writes** (`apply.ts:814-850` `span.carry`):
  the reference's writer knows the spans it edited, so lines outside a span keep
  their anchors by offset and one line inside a span may carry the span's first
  anchor under a checksum check — `O(spans + edited lines)`, aligning nothing.
  tau re-derives the whole file through `carry`; the prefix/suffix trim keeps
  the common case (a touched window inside an otherwise unchanged file) cheap,
  with a quadratic worst case on a wholesale rewrite. Cheaper and exact for the
  app-write path. The registry's path — content changed out of band, no span
  knowledge — genuinely needs the alignment, so G7 does not retire `carry`.
## 5. Method

Pro side: `read.ts`, `edit-common.ts`, `hashline/{parse,resolve,apply}.ts`,
`anchor-registry.ts`, `batch.ts`, `served.ts`, `line-endings.ts`,
`normalize.ts`, `constants.ts` read in full where behavior-bearing. Tau side:
`tools/impls.rs` (read/edit), `hashline/{mod,registry,line_endings,encoding,
edit_diff,pin_store}.rs` on `main` d339ff6. Every row above is cited on both
sides; rows marked "at parity (#285)" were verified against the pro sources in
the PR #285 review (two independent reviewer passes).
