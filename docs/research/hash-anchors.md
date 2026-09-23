# Research: Dirac-style hash anchors (word anchors + stateful manager + Myers re-anchoring)

Issue: [#33](https://github.com/aaronlockhartdev/tau/issues/33) · Researched 2026-09-22 (working tree, no branch)

**Question.** The Dirac post proposes a specific hash-anchored editing mechanism — single-token word anchors from a ~1,700-entry tiktoken `o200k_base` list, `§` delimiter, a stateful task-scoped anchor manager, and Myers-diff re-anchoring after every edit — benchmarked at 8/8 and ~$0.18/task vs $0.38–0.73 for other harnesses. Settle it for tau: token math for tau's read-heavy/edit-moderate workload, anchor-vocabulary interaction with model attention, failure modes (external edits, concurrent sub-agents, session resume), state lifetime — and adopt / modify / skip.

**Short answer.** **Skip the Dirac variant; keep tau's current scheme.** Tau already ships hash-anchored read/edit (spec §5.4, ticket #19, `hashline.rs`) — a port of Can Bölük's allocated-hash scheme, which has *controlled* 16-model evidence behind it (+15 pts, −17…−61% output tokens). Dirac's three distinguishing elements do not transfer: (1) the 1-token word property is **encoding-specific** — it holds on `o200k_base` (verified: all 1,721 words are 1 token) but only ~75% on `cl100k_base`, so the read-overhead saving is 1.1–1.2 tokens/line, worth ~$0.004–0.04 per heavy session; (2) the **stateful task-scoped manager with random per-task allocation conflicts with tau's stateless multi-session architecture** — in tau, parent + sub-agents + GUI are first-class sessions over one workspace, and content-derived anchors are the only anchor scheme all of them share; (3) Dirac's **edit endpoints are the complete `ANCHOR§CONTENT` line verbatim** (pattern-enforced in the schema), which costs *more* output tokens than tau's bare 3-char `{from, to}` for equal validation. Myers re-anchoring's stability benefit is also measured to be already present: tau's stateless re-derivation churns an average of ~2.2 anchors (max 6) after a single-line external edit, on duplicate-heavy files. Two cheap Dirac borrowings are noted as follow-up candidates (§7): an unchanged-reread dedup response, and opt-in anchoring on `read`.

---

## 1. What tau has today (the "current anchor model")

The issue's premise — "the current tau `edit` tool is search-and-replace over served line content" — predates ticket #19. As of today, tau's `read`/`edit` are **already hash-anchored** (`crates/tau-core/src/hashline.rs`, spec §5.4):

- **Read format**: every line is `HASH│content` — a 3-char anchor from a 62-char alphabet (A–Z, a–z, 0–9), `│` (U+2502) delimiter. Paging via `offset`/`limit`; truncated reads end with `… truncated (showing lines X–Y of Z)`.
- **Uniqueness is allocated, not hashed**: a per-file bitset over the full 238,328-slot space with collision probing (stride 3907 = 62²+62+1). A line's base slot is `xxh32(canon) >> 14 mod 62³`; colliding lines probe. An anchor is unique in its file by construction.
- **canon** = whitespace-stripped line (reformatting never invalidates); CRLF normalized to LF on read and through edit.
- **Edit contract** (model-facing, `tools.rs`): `{path, from, to, content}` — `from`/`to` are bare 3-char anchors; the inclusive range is replaced. Stale/ambiguous anchors are **rejected, never fuzzy-matched**; the diagnostic tells the model to re-read.
- **Re-derivation, not a manager**: `apply_edit` recomputes the anchor set from the file's *current* content, then `stable_hashes` reuses survivors' old anchors (matched by canon + nearest position) and allocates fresh anchors only for new lines. **There is no persistent anchor state** — the file on disk is the single source of truth, and identical content always yields identical anchors (deterministic per content).
- **Edit response**: the changed region plus two context lines per side, with fresh anchors, plus `edited <path> (lines X–Y of Z)`.
- **Cap**: 238,328 lines → fall back to `write`; whole-file emptying via `edit` is rejected.

This is the Can Bölük scheme (his format was `1:a3|` — line number + 2-char content hash; tau dropped the line number, which is exactly the part Dirac's post criticizes as invalidating all subsequent lines after an upstream edit).

## 2. How Dirac actually works (from the repo, not the post)

Primary source: [dirac-run/dirac](https://github.com/dirac-run/dirac), shallow-cloned 2026-09-22. Several post claims differ from the current implementation — the repo wins:

**Anchors.** `src/utils/.hash_anchors`: **1,721 words**, all matching `^[A-Z][a-zA-Z]*$` (capital-starting, letters only): `Inflater`, `Normalizer`, `Popover`, `Spawner`, `Quaternion`, `Firebase`, `Pagination`… — i.e. mostly programming-flavored terms. Verified with tiktoken: all 1,721 are **exactly 1 token** in `o200k_base`.

**Allocation is random and per-task.** On first read of a file, `AnchorStateManager.reconcileWithChanges` shuffles the dictionary with `Math.random()` and pops words in order. **Two tasks reading the same file get different anchors for the same line.** Fallback when the 1,721 are exhausted: 2-word concatenations (`ParameterDriver` — 2 tokens in 200/200 `o200k_base` samples), then 3-word (3 tokens), pool refilled up to 10,000. The post's "2-token anchors" is this.

**Delimiter** `§` (1 token in `o200k_base` and `cl100k_base`).

**Read is opt-in.** `read_file` takes `include_anchors: true`; without it, lines are plain. Anchored reads call `env.anchors.reconcile(path, allLines)` (so external changes are picked up on read itself) and *always* return full anchored content. Unanchored whole-file re-reads of an unchanged file return a one-line dedup: `no changes have been made to the file since your last read (Hash: …)` (content-hash-cached in task state).

**Edit endpoints are whole lines, not bare anchors.** `edit_file` takes `{files: [{path, edits: [{edit_type, anchor, end_anchor, text}]}]}` where `anchor`/`end_anchor` are pattern-validated as **the complete `ANCHOR§CONTENT` line** (`getAnchoredLinePattern`: `^[A-Z][a-zA-Z]*§[^\r\n]*$`), "copied verbatim"; the backend locates by anchor word and "verifies the supplied content exactly" (`EditExecutor.resolveAnchor`). The model thus re-emits *both endpoint lines in full* per range edit. `edit_type` ∈ `replace | insert_after | insert_before`; edits batch across files with **per-edit partial success** (failed edits reported individually, others applied); replacement text containing an anchored-looking line is rejected (`containsAnchoredLine`); overlapping edits rejected.

**Reconciler.** `AnchorStateManager` (in-memory, `static Map<taskId, Map<path, TrackedDocument>>`; caps: 1,024 files/task, 50 tasks) stores per-line **FNV-1a 32-bit** content fingerprints. On any read/edit, changed files are diffed with `diff.diffArrays` (the jsdiff package — Myers) over the fingerprint arrays; only added lines get new words, survivors keep theirs. A chokidar `FileContextTracker` watches read files and forces a re-read before edits if the file changed outside Dirac.

**State lifetime: persisted per task, not session.** `DiracContext` stores the full `PersistedAnchorState` (version 1: path + hashes + anchors + usedWords + availablePool per document) in the task's tool-context store at `~/.dirac/data/tasks/<taskId>/` — baseline + append-only operation log (the same event-sourced pattern tau uses for sessions), with `hydrate`/`export` for exact restore across process restarts. Task-scoped by construction: a fresh task on the same file re-shuffles and re-allocates.

**Over-limit files**: 50,000 lines **or** 20 MiB → no anchors at all; "use execute_command" (vs tau's 238,328-line cap → `write`).

**Response**: the edit response carries a unified-style diff (±3 context lines, `getDiffBlock`) over **raw, unanchored** lines. The post's claim "we send back the updated anchors in the response" does not match current HEAD — re-anchored lines reach the model on the *next anchored read* (reconcile-on-read), not in the edit response.

**The benchmark.** [Post](https://dirac.run/posts/hash-anchors-myers-diff-single-token): 8 edit-heavy refactoring tasks (3–25 files each; `evals/README.md`: transformers ×4, vscode ×3, django ×1; `git reset --hard && git clean -fd` before each run). Dirac 8/8 at **$0.18 avg** vs Cline $0.49 (5/8), Kilo $0.73 (5/8), Ohmypi $0.51 (6/8), Opencode $0.44 (**8/8**), Pimono $0.38 (6/8), Roo $0.60 (6/8). Two caveats: it is a **cross-harness** comparison (different prompts, tools, and unstated per-harness models), and **correctness is not Dirac-specific** (Opencode also 8/8, at 2.4× the cost). The only *controlled* evidence for anchor mechanisms is Can Bölük's A/B below.

## 3. Token math for tau's workload

### 3.1 Measured per-token costs (tiktoken, 2026-09-22)

| | `o200k_base` (GPT-4o/O-series) | `cl100k_base` (GPT-3.5/4) |
|---|---|---|
| tau anchor (3-char, n=2,000 samples) | 1–3 tok, avg **2.23** | 1–3 tok, avg **2.34** |
| tau row overhead (anchor + `│`) | **≈3.23 tok/line** | **≈3.34 tok/line** |
| Dirac word (1,721 dict) | 1 tok (1721/1721) | 1 tok (1276), 2 tok (432), 3–4 tok (13) → avg 1.27 |
| Dirac row overhead (word + `§`) | **2.0 tok/line** | **≈2.27 tok/line** |
| `│` / `§` delimiter | 1 tok | 1 tok |

Both delimiters cost the same; the entire read-side delta is the anchor label: **1.07–1.23 tokens/line** in Dirac's favor, and the advantage shrinks on non-o200k encoders because the dictionary is curated for `o200k_base` (tau's provider model — ADR-0003 — is any OpenAI-compatible endpoint).

### 3.2 Session-level arithmetic

Cost basis (current mainstream list prices; output 5–6× input, ~50× cached — the post's own premise): input $3/M, output $15/M, cached input $0.30/M.

Read-heavy profile per the issue: a session reads ~20 files of ~500 lines (~4,000 content tokens each), edits ~5 ranges of ~20 lines (R ≈ 160 output tokens).

| | tau today | Dirac scheme | Δ (Dirac − tau) |
|---|---|---|---|
| Read overhead (20 files) | 20 × 1,615 = **32.3k input tok** ≈ $0.097 uncached / $0.0097 cached | 20 × 1,000 = **20.0k input tok** ≈ $0.060 / $0.006 | **−12.3k tok ≈ −$0.037 / −$0.004** |
| Edit output (5 edits) | 5 × (160 + ~5 anchors) ≈ 825 tok | 5 × (160 + 2 × ~18 endpoint lines + 4) ≈ 985 tok | **+160 tok ≈ +$0.0024** |
| Net per session (o200k-class model) | | | **Dirac ~$0.03 cheaper uncached / ~$0.004 cached** |

On a `cl100k`-class model the read delta shrinks to ≈1.07 tok/line (≈$0.03/$0.003). Conclusions:

1. **The word dictionary is a read-side micro-optimization** — single-digit cents per heavy session, less on cached input, encoding-dependent. It is not where the Dirac benchmark's 2–4× gap comes from.
2. **The edit side, if anything, favors tau**: bare 3-char endpoints (~5 tokens) vs Dirac's two full endpoint lines (24–40+ tokens). Both are O(R) in the replacement body; Dirac's whole-line "validator" buys stronger content validation at a real output-token price.
3. The Dirac post's headline 4-token/line overhead figure describes the *old* line-numbered hashline (`101:x9|`), not its own scheme; against that baseline both schemes are ~2 tok/line (Dirac) vs ~3.2 (tau).
4. The **cross-harness $0.18 vs $0.38–0.73 does not transfer to tau**: different prompts/tools/models, and Opencode matches Dirac's 8/8 at $0.44. What does transfer is the *mechanism-level* controlled evidence: Can Bölük's A/B (same harness, edit tool only changed — [post](https://blog.can.ac/2026/02/12/the-harness-problem/), cited by spec §5.4): 16 models, 3 runs × 180 tasks, fresh session each; hashline beats patch by **+15 pts average, 14/16 models**, **−17%…−61% output tokens**, up to 10× pass-rate (Grok Code Fast 1); str_replace/patch failure rates of 46–51% on non-native models. **Tau already captures that evidence.**

Workload data note: the repo's local `.tau/sessions/` (16 files) are early smoke tests (8 bash / 4 read / 3 write / 1 edit; 72.5k input / 2.4k output tokens total) — not representative, so the arithmetic above uses the issue's stated read-heavy/edit-moderate shape (matching the Dirac post's premise 1: "reads outnumbering edits by a significant margin") rather than measured tau traffic.

## 4. Anchor vocabulary × model attention

- **No controlled A/B exists** (as of this research) comparing word anchors vs opaque short labels on the same harness. The attention question is therefore not settled empirically in either direction.
- The strongest available evidence is that **opaque 2–3-char labels work across 16 models** (Can Bölük A/B, §3.2) — including models that fail patch/str_replace at 46–51%. A 3-char alphanumeric is as "alien" to prose/code as a CamelCase word; models copy both fine (this very research session was conducted through tau's current `HASH│`/`{from,to}` contract without a single anchor-related failure).
- **Content-collision surface**: Dirac's words are *content-like* — programming terms drawn from the code domain (`Inflater`, `Quaternion`, `Firebase`). A content line mentioning "Normalizer" next to an anchor line `Normalizer§…` forces position-based disambiguation (anchor only valid at line start + `§`). Dirac guards the write side (`containsAnchoredLine` rejects replacement text containing anchored-looking lines) but a *read* of a file whose own line starts with `<DictWord>§` would parse as anchored (a `§` at that position in real code is near-impossible, so the practical risk is low). Tau's labels can appear in code too (hex constants, identifiers like `abc`), but the line-start + `│` shape is equally rare in source. Net: both schemes have a small, similar collision surface; neither is clearly attention-safer.
- The one defensible attention-related difference is **1 token = less room to mistype**: a single-token word leaves the model no token-split ambiguity (e.g. `Inflater` can't split; `wUp` is 1–3 tokens depending on encoder, and a 3-token anchor gives three copy-failure opportunities). This is the real, if small, argument for the dictionary — and it is a *label-encoding* argument, independent of any stateful machinery.

## 5. Failure modes

| Failure | Tau today (stateless allocation) | Dirac (stateful task manager) |
|---|---|---|
| **In-file collision** | Impossible — uniqueness by allocation (238,328 space). | Impossible within a file — `usedWords` set; beyond 1,721 lines falls back to 2–3-token concatenations. |
| **External edit mid-task** | Next read/edit re-derives from disk; stale anchors rejected with a "re-read" diagnostic. **Measured churn** (Rust port of `line_hashes`, duplicate-heavy lines — blank, `}`, repeated patterns): single-line external edit → **avg 2.2 anchors change, max 6** (500- and 5,000-line files; exactly 1 on collision-free content). i.e. essentially Myers-quality without a state manager. | chokidar watcher + Myers reconcile on read/edit; only added lines get new words. Extra machinery: watcher lifecycle, persist/hydrate, and a stale-state surface if a watcher event is missed (reconcile-on-read catches it, so worst case = one wasted read, same as tau). |
| **Concurrent workers on one file** | Two sub-agents editing the same file: last writer wins (`write_atomic` rename); the loser's anchors are stale → rejected → re-read → retry. Because anchors are **content-derived, both workers see the same anchors for the same content** — the retry loop is well-defined and the parent/GUI see the same coordinates. | Per-task state: two tasks reading one file hold **disjoint word spaces** (random shuffle). Each reconciles against its own map by content hash, so a survivor line re-maps and a deleted line breaks its anchor cleanly — mostly safe, but the two sessions' contexts cannot share anchor vocabulary; a parent resuming a worker session cannot use the worker's anchors. |
| **Session resume** | **No state to lose.** Anchors in a pre-resume context stay valid if the file is unchanged (deterministic per content); changed lines are stale and self-diagnosing. Survives branch/fork/GUI-tab-open for free. | State persisted per *task* (v1, `~/.dirac/…/tasks/<id>/`) and re-hydrated on restore — works within Dirac's one-task-per-conversation model. In tau's model (sub-agents are ordinary sessions, nothing auto-resumes, sessions branch), a task-scoped manager would need per-session persistence + merge rules on fork — machinery with no matching benefit, since tau's anchors need no persistence at all. |
| **Over-cap file** | 238,328 lines → `write` fallback (rare in practice). | 50,000 lines **or 20 MiB** → anchors unavailable, "use execute_command" — a real operational edge for large generated/bundled files. |
| **Out-of-vocabulary** | N/A — the space is combinatorial, not a list. | After 1,721 words/file, 2-token concatenations (verified 2/3 tokens on `o200k_base`): read overhead rises to ~3–4 tok/line for the tail of long files. |

## 6. State lifetime

- **Dirac**: per **task**, in-memory + persisted (baseline + operation log, version 1; 1,024 files/task; 50 tasks resident; exact hydrate). A fresh task = fresh random allocation. Lifetime ≈ a conversation.
- **Tau has none, by design**: the anchor set is a pure function of current file content, computed per read/edit. Lifetime = the file. That is the property making tau's session architecture work — parent, N concurrent sub-agents, the GUI, resume, and fork all agree on coordinates with zero synchronization. The `CONTEXT.md` session model (first-class, openable, branchable sub-agent sessions; "nothing auto-resumes"; resume contract as a plain summary) has no slot for a task-scoped anchor store, and adding one would force *every* session to re-read files to recover coordinates after any restart — precisely the failure the stateless scheme avoids.
- **The post's "statefulness is required" claim rests on its anchor choice**: because words are assigned arbitrarily (random shuffle), the mapping can't be recomputed from content. It would become re-computable — and thus stateless — if allocation were deterministic (e.g. dictionary slot = `xxh32(canon)` base index into the 1,721-word list, probing on collision). That variant keeps cross-session consistency *and* the 1-token label; it is the only version of the word-idea compatible with tau.

## 7. Recommendation

**Skip the Dirac variant; keep the current allocated-hash scheme.** The mechanism is already adopted (that's what §5.4 is); the question is whether Dirac's specific deltas earn their cost. They don't:

1. **Word anchors** — adopt only as an optional label-encoding micro-optimization, and only in deterministic-allocation form (§6): ~1.1–1.23 tok/line read saving ≈ **$0.004–0.04 per heavy session** (less on cached input, less on non-o200k encoders), with a small content-collision surface and an untested attention argument. Not worth a v0 change; worth a ticket if real session cost data shows anchor overhead is material. The dictionary asset itself (1,721 `o200k_base` single-token words) is freely usable from the Dirac repo if that day comes.
2. **Stateful task-scoped manager + Myers reconciler** — **do not adopt**. It conflicts with multi-session anchor sharing (§5, §6), and the churn measurement (§5) shows tau's stateless re-derivation already achieves near-Myers stability at realistic file sizes.
3. **Whole-line edit endpoints** — **do not adopt**: strictly more output tokens than `{from,to}` for weaker ergonomics (the model must copy two full lines verbatim).

**Two cheap borrowings worth follow-up tickets** (each orthogonal to the anchor mechanism; neither is this issue's scope):

- **Unchanged-reread dedup**: when a whole-file `read` returns content identical to the last served read of that file in this session, return a one-line `no changes since your last read` instead (Dirac's `fileHashes` cache). For a read-heavy workload this is the single biggest per-read saving available, and it composes with the existing scheme unchanged.
- **Opt-in anchoring on `read`** (`include_anchors`): pay the ~3.2 tok/line only when the model intends to edit; plain reads for context stay plain. Trade-off: two read modes to teach the model; the current "always anchored" contract is simpler and the Can Bölük evidence covers it.

Cost summary backing the recommendation: the anchor *paradigm* is worth +15 pts and −17…−61% output tokens (controlled, 16 models) — already in tau. Dirac's *deltas* are worth ~$0.004–0.04 per heavy session on the read side, negative on the edit side, and require a state architecture tau's session model explicitly avoids.

---

## Sources

Retrieved 2026-09-22.

- Dirac: [post](https://dirac.run/posts/hash-anchors-myers-diff-single-token) (scraped); [dirac-run/dirac](https://github.com/dirac-run/dirac) @ HEAD (shallow clone): `src/utils/.hash_anchors` (1,721 words), `src/utils/AnchorStateManager.ts`, `src/shared/utils/line-hashing.ts`, `src/shared/anchor-limits.ts` (50k lines / 20 MiB), `src/core/task/tools/context/DiracContext.ts` (per-task persistence), `src/core/task/tools/modules/read_file/ReadFileTool.ts` (`include_anchors`, dedup cache), `src/core/task/tools/modules/edit_file/` (`EditFileTool.ts` schema, `EditFileValidator.ts`, `EditFileBatchPreparer.ts`, `EditFileApplier.ts`, `utils/EditExecutor.ts`, `utils/EditFormatter.ts`), `src/core/context/context-tracking/FileContextTracker.ts` (chokidar), `evals/README.md` (the 8 tasks).
- Can Bölük, [The Harness Problem](https://blog.can.ac/2026/02/12/the-harness-problem/) (the controlled 16-model A/B; cited by spec §5.4).
- tiktoken (`o200k_base`, `cl100k_base`) — anchor/word/delimiter tokenization, measured 2026-09-22 (2,000 random 3-char anchors; full 1,721-word dictionary; 200 samples of 2-word/3-word concatenations).
- Anchor-churn experiment: Rust port of `hashline` allocation (`/tmp/churn`, this research, not committed) — 500/5,000-line duplicate-heavy files, single-line external edit.
- tau: `crates/tau-core/src/hashline.rs`, `crates/tau-core/src/tools.rs` (tool specs, `read`/`edit`/`write`/`write_atomic`), `docs/spec/v0.md` §5.4, `CONTEXT.md`, local `.tau/sessions/` (smoke-test data only).
