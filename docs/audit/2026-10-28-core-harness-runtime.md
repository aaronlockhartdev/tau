# core::harness runtime audit (2026-10-28)

Scope: `crates/tau-core/src/harness/*.rs` runtime files (non-`tests_*`, non-`testkit`) plus `harness/git/` — 5,732 LOC by `wc -l` (5,595 excluding `git/tests.rs`). Runtime files audited: `mod.rs`, `state.rs` 721, `launch.rs` 653, `dispatch_sessions.rs` 524, `dispatch.rs`, `dispatch_commands.rs` 282, `sessions.rs` 476, `sessions_archive.rs` 278, `snapshot.rs` 408, `forwarding.rs` 471, `watchers.rs` 315, `watch.rs`, `turn.rs` 256, `pump.rs`, `thinking.rs`, `session_open.rs`, `git/mod.rs` 425, `git/ignore.rs` 135.

Baseline:
- `cargo clippy -p tau-core --all-targets` — pass, 0 warnings (14.7s).
- `cargo doc -p tau-core --no-deps` — pass, 2 warnings, both OUT of scope (`tools/progress.rs:26`, `:117`, unresolved link `report`).
- `rg` is not installed on this box; probes below use `grep -rn` over `crates/ app/src-tauri/ app/svelte/ app/tests/ fuzz/ eval/`.
- Vitest/ESLint not run: scope is Rust harness.

## A — delete/fix now

1. **Dead computation: untracked numstat.** `crates/tau-core/src/harness/git/mod.rs:229` inserts `numstat[path] = (count_lines(...), Some(0))` for every untracked file, and `count_lines` (`git/mod.rs:351`, 17 LOC) reads up to 100k lines per file. `git_state_of` (`git/mod.rs:397`) returns `added: None, deleted: None` for the Untracked arm (`git/mod.rs:412-416`); only the Modified arm reads numstat. The untracked entries are never consumed by anyone (probe in Dead-code evidence). Cost: a full read of every untracked file on every snapshot rebuild. Fix: delete the untracked loop + `count_lines`, and fix the doc at `git/mod.rs:183-185` ("untracked files are measured here … so neither the tree nor the changes tab does per-row file reads") which states the opposite of what the code does. −25 LOC.
2. **Preview truncation mixes bytes and chars.** `snapshot.rs:330` truncates with `first.chars().take(80)`; `snapshot.rs:331` appends `…` when `first.len() > 80` (bytes). A 79-char multibyte line gets a spurious ellipsis; an 85-char/80-byte line does not. Fix: compare `chars().count()` (or truncate and compare in one unit). 2 LOC.
3. **Post-turn re-emit regresses blob inlining.** `turn.rs:176` re-emits the tool entry with `entry_to_view(entry)`, while live hooks emit through `UpsertView` (`launch.rs:538`) which inlines sidecar-blob payloads into the upsert (#265). Settlement can rewrite a live card that had inline text into a blobless view; the frontend recovers via `blob_read` (`app/svelte/lib/store/entries.ts:113-115`), so the cost is a re-fetch flicker, not a bug — but the two emit paths should agree. Fix: use the same `UpsertView` (blob root is reachable from `live`).
4. **Memory diverges from the file on write failure (record-is-the-file violation).** `dispatch_sessions.rs:65` sets `l.meta.lock().title = Some(title)` for every live session BEFORE `store.set_title` at `dispatch_sessions.rs:93`; if the file write fails, in-memory titles disagree with the record (CONTEXT.md: the file is the record). Same class: `SessionSetModel` mutates `live.meta` and calls `live.agent.set_model` (`dispatch_sessions.rs:115`) before the `diagnostic(ModelChanged)` append at `:116-123`. Fix: write first, mutate on success.
5. **Job-hook message interpolates `{output}` twice.** `launch.rs:602`: `"background job {job_id} ({output}): finished with exit {status}; output: {output}"` — the parenthetical is the full job output pasted into the prompt, unbounded, twice. Cost: prompt bloat on every job completion; a job with a large log injects it two times. Fix: preview in the parenthetical (or drop it), keep one `output:` section.
6. **Doc comments that lie.** `state.rs:489-493`: the doc above `load_workspace_index` describes the *skill registry* (ticket #28/#31) — a stale copy-paste above an unrelated fn. `sessions.rs:471`: `/// branch) and the delete: the same guard as the archive (ADR-0005).` is a truncated fragment above `running_turn_refusal`. Fix/rewrite, 2 lines each. (AGENTS.md comment rule: a comment is a why, not a stale what.)
7. **Nested-.gitignore walk runs on every snapshot, and snapshots die on every tree change.** `git/ignore.rs:34-78` (45 LOC) recursively walks the ENTIRE worktree collecting nested `.gitignore` files (it skips ignored dirs, so `target/` is not walked — but a big source tree is). `watchers.rs:101` removes the git snapshot on ANY tree change, so the next `FileList` pays the full walk again. On a repo like this one that is a worktree scan per editor save-batch, on the dispatch path. Fix: bound the walk (depth/count cap), or invalidate per changed dir, or cache the collected paths and re-stat (see Offload for the gix route).
8. **Unbounded per-session provider bookkeeping.** `forwarding.rs:27` `calls: Arc<Mutex<Vec<String>>>` and `forwarding.rs:31` `completed: Arc<Mutex<HashMap<String,bool>>>` grow for the life of a session — every stream call appends, nothing prunes (`turn.rs` reads them with `calls_before` offsets and never drains). Long-lived root sessions accumulate every call id ever made. Fix: prune entries below the current turn's `calls_before` watermark after each settlement. ~5 LOC.

## B — simplify

1. **Wake sequence triplicated 3×.** send_notified_steer → emit Queue → reset stop flag → CAS `turn` → `tokio::spawn(run_turn)`: `launch.rs:575-618` (job hook), `forwarding.rs:409-457` (`wake`), `dispatch_commands.rs:55-86` (`MessageSend`). One helper (`Core::wake(live, text)`) owns it; ~40 LOC collapsed.
2. **`(since, range)` projection duplicated.** `snapshot.rs:192-197` (`entries`) and the same match in `entries_from_disk` (`snapshot.rs:267+`), ~20 LOC each. Extract the cursor/range → entries mapping.
3. **`ProtocolError::Other { message: e.to_string() }` boilerplate: 77 sites** in scope files (`grep -c "ProtocolError::Other" $(scope files)` = 77). A `From<io::Error>`/`From<session::Error>` for `ProtocolError`, or one `other(e)` helper, collapses ~60 LOC of noise (typed-IPC rule is kept — the helper produces the tagged value).
4. **Subagent command arms repeat the handle→live→supervisor pipeline.** `dispatch_commands.rs:145-276`: SubagentList/State/Spawn/Message/Stop/Activity each do `child_parent(handle)` → `sessions.lock().get(parent)` → `sup.subagents()` → action → `state_info` → `info_to_protocol`. One `with_subagent(handle, action)` helper; ~20 LOC.
5. **Re-attach of a detached session repeats 5×.** `sessions_archive.rs:56, 79, 110, 134, 141` — `self.sessions.lock().insert(session.to_owned(), live.clone())` inside every abort path. One `reattach(&self, id, live)`.
6. **`session {id} is not open` built by hand 7×**: `dispatch_sessions.rs:74, 159`; `sessions_archive.rs:166, 242`; `state.rs:663`; `snapshot.rs:217, 277`. Helper `closed_session(id)`; keeps the message (tested by `tests_model.rs:114`) in one place.
7. **`live_subagents` is a forwarding wrapper.** `snapshot.rs:84-92` — 6 lines around `sup.fleet_view()` whose doc repeats the "#250 §6.6 union" explanation that lives at the SubagentList arm (`dispatch_commands.rs:145`). Inline it, single caller.
8. **Sentinel magic paths.** `/nonexistent-tau-project` at `state.rs:264` and `dispatch_commands.rs:135`; `/nonexistent-tau-system` at `state.rs:654`. Three hand-written filesystem sentinels meaning "no config layer here" — a named `const NO_PATH: &Path` (or an `Option`) in one place.
9. **Three runtime files over the 500-LOC soft limit** with multiple reasons to change: `state.rs` 721 (build + registry + config layering + plan + pipe), `launch.rs` 653 (roles + root assembly + event hooks + UpsertView), `dispatch_sessions.rs` 524. Each invites a seam split along a domain (config layering out of state.rs; hooks out of launch.rs).
10. **Six production `#[allow(clippy::too_many_lines)]` with the same speculative justification** ("splitting is refactoring"): `turn.rs:33`, `dispatch_commands.rs:9`, `dispatch_sessions.rs:12`, `sessions_archive.rs:20`, `launch.rs:89`, `launch.rs:286`. The suppressions mark exactly where B9's splits belong.
11. **Test-only builder seams, one of which forks a production path.** `state.rs:189 with_child_factory`, `:197 with_root_provider`, `:204 with_system_dir`, `:211 with_home` — documented seams (acceptable per rubric), prod callers = 0 (probe below). `with_root_provider` is the sharp one: `build_live` branches on `Core.root_provider: Option<TurnProviderRef>`, a production code path existing only for tests. Collapse by resolving the provider in the same place tests can inject (the factory seam already exists).
12. **`count_lines` offload (see Offload):** replacing it with `Read::lines_count` (stable since 1.85; workspace rust-version 1.85) removes a hand-rolled read loop — pair with A1.
13. **Over-broad `pub` inside crate-private modules.** `harness/mod.rs:68-73` is the real public gate (only `Core`, `CoreBuilder`, `CoreError`, `SessionRole`, `pump`). These `pub` items have zero users outside `harness/` (probe in Dead-code evidence): `git/mod.rs:124 rebase_status`, `:145 git_show`, `:174 NumstatCounts`, `:181 GitSnapshot`, `:203 git_snapshot`, `:369 annotate` (all under the private `mod git`), `watch.rs:32 Batch`, `watch.rs:34 Watcher`. Narrow to `pub(super)`/`pub(crate)`: 8 visibility decls, no behavior change. (`SessionRole` is genuinely public — `crates/tau-test/src/main.rs:23`, `tests_accept.rs`.)
## C — smell

1. `launch.rs:358` and `launch.rs:440` — `let sid = store.id().to_owned();` re-bound in the same assembly (also `:144`, `:209` in the role arms).
2. `launch.rs:538, 539` — `UpsertView::new(blob_root.to_path_buf())` constructed twice, identically.
3. `launch.rs:417` and `:454` — `gc_policy(config)` computed twice in one root assembly.
4. `launch.rs:359` — `plan_wiring(Some(core), …)` destructures `let (mut system_prompt, _) =`, discarding the returned `Option<Arc<Core>>`; the core is then rewired by `wire_events`' plan hook. Two paths wire the same plan hook; the fn returns a value only the other call sites use.
5. `state.rs` `workspace_config` caches merged config per workspace id (`self.configs`) with no invalidation on `config.toml` edits (the watcher refreshes skills, not config) — defensible (skills freeze at open too) but undocumented.
6. `dispatch.rs:131, 205`, `dispatch_commands.rs:279`, `dispatch_sessions.rs:521` — `_ => unreachable!("dispatch routes the arm")`: the type system does NOT exclude these (every `Command` variant exists; the router just never routes it here), so a new `Command` variant compiles into a runtime panic instead of a match-exhaustiveness error at the router. Prefer `ProtocolError::Unsupported` or exhaustive sub-matches.

## Clean

- **Panic audit passes.** `grep -rn 'unwrap()\|expect(\|panic!\|unreachable!' crates/tau-core/src/harness/*.rs` (excluding `tests_*`/`testkit`/`git/tests.rs`) leaves exactly 8 sites: the 4 router `unreachable!` arms (C6) and 4 `expect("<invariant>")` — `launch.rs:522` (queue hook lives in the session it captures), `sessions.rs:265` (`self_arc` set at build), `state.rs:297` (`names::Generator` never exhausts), `state.rs:429` (pump `take()`s the receiver exactly once, sole consumers `harness/pump.rs:9` + `tau-acp/pump.rs:38`). All match the AGENTS.md `expect` rule (stated invariant, non-foreseeable path); `watch.rs` unwraps are `#[cfg(test)]` (line 127+).
- **No production-dead code in scope** (see Dead-code evidence): every `pub` fn/type in the runtime files traces to a live caller; the only dead *work* is A1.
- **Gate-clean**: clippy 0 warnings on all targets; no in-scope rustdoc warnings.
- Structural spine is sound: single dispatch router (`dispatch.rs`) → per-domain files; single construction path (`launch.rs` R2); settlement guarded on every exit (`turn.rs`). The B-list is boilerplate collapse, not architecture.

1. `launch.rs:358` and `launch.rs:440` — `let sid = store.id().to_owned();` re-bound in the same assembly (also `:144`, `:209` in the role arms).
2. `launch.rs:538, 539` — `UpsertView::new(blob_root.to_path_buf())` constructed twice, identically.
3. `launch.rs:417` and `:454` — `gc_policy(config)` computed twice in one root assembly.
4. `launch.rs:359` — `plan_wiring(Some(core), …)` destructures `let (mut system_prompt, _) =`, discarding the returned `Option<Arc<Core>>`; the core is then rewired by `wire_events`' plan hook. Two paths wiring the same plan hook; the fn returns a value only the other call sites use.
5. `state.rs` `workspace_config` caches the merged config per workspace id (`self.configs`) with no invalidation on `config.toml` edits (watcher refreshes skills, not config). A running workspace keeps stale config — behaviorally defensible (matches skills freezing at open), but undocumented.
6. `dispatch.rs:131, 205`, `dispatch_commands.rs:279`, `dispatch_sessions.rs:521` — `_ => unreachable!("dispatch routes the arm")` in the sub-dispatchers: the type system does not exclude these (a `Command` variant exists; the router just never sends it here), so the panics are real branches — a new variant added to `Command` compiles into these panics instead of a match-exhaustiveness error at the router. Prefer returning `ProtocolError::Unsupported` or restructuring to exhaustive matches.

## Offload candidates

- `collect_nested_gitignores` — `git/ignore.rs:34-78`, 45 LOC of hand-rolled worktree walk collecting nested `.gitignore` files to feed `gix::ignore::Search`. **gix-ignore 0.23 (already a dependency, `Cargo.toml:29`, APL-1.0/MIT — shipped today)** owns this in its status/dir walks: `gix-status`' `index_worktree::iter::Source::Real { source }` and `gix-dir`' `Source::Untracked` load nested gitignores along the walk and yield the source paths. Migration risk: M — the tree-listing UI needs the *set of all* nested patterns (not per-path lazy), and gix's API is walk-scoped; verify the emitted sources are a superset of what `is_ignored` needs. Meets the >50-LOC bar jointly with the walk it replaces (A7).
- `head_blob` (`git/mod.rs:303-319`, 17 LOC, recursive tree-segment walk "gix has no find_file") and `collect_tree_paths` (`git/mod.rs:325-347`, 23 LOC, recursive HEAD-tree walk) — gix object/tree APIs own path lookup (`Tree::lookup_entry`/`Path::lookup`) and index traversal. **Below the 50-LOC wheel bar; note only.** gix 0.88 / gix-object 0.65 (`commit.time()` verified) is the owner.
- `count_lines` (`git/mod.rs:351-367`, 17 LOC) → `std::io::Read::lines_count` (stable 1.85; workspace rust-version 1.85) + `Read::take` for the cap. No new dependency. Mostly mooted by A1 (delete the caller).
- Keep as-is (already offloaded): `line_diff_counts` → `similar`; `git/` status → gix (ADR-0011).

## Dead-code evidence

All probes run with `grep -rn` (no `rg` on this box) over `crates/ app/src-tauri/ app/svelte/ app/tests/ fuzz/ eval/`; "prod" excludes files named `tests*`/`testkit`.

- `count_lines` → prod=2 (def `git/mod.rs:351` + call `git/mod.rs:229`), fe=0. Consumed value provably dropped: `git_state_of` Untracked arm `git/mod.rs:412-416` returns `added: None, deleted: None`; Modified arm `git/mod.rs:418` reads numstat. → A1.
- `with_child_factory` prod=1 (def; 20 test uses), `with_root_provider` prod=1 (def; 1 test), `with_system_dir` prod=1 (def; 16 test), `with_home` prod=1 (def; 9 test) → B11.
- `live_subagents` prod=2 (def + 1 use in `snapshot.rs`) → B7.
- Visibility probe (B13): `grep -rn "\bNAME\b" crates app/src-tauri fuzz eval --include='*.rs' | grep -v 'crates/tau-core/src/harness/' | wc -l` → `rebase_status` 0, `git_show` 0, `git_snapshot` 0, `GitSnapshot` 0, `NumstatCounts` 0, `annotate` 1 (a prose word in `provider/stream.rs:278`, not a use), `Watcher` 0, `Batch` 0; `SessionRole` 36 (real: `crates/tau-test/`).
- No production-dead functions found in scope. Notably LIVE (do not report): `pump::pump` (`app/src-tauri/src/main.rs:170`), `lane_to_message_lane` (`crates/tau-core/src/agent/queue.rs:179`), `Command::BlobRead` (frontend `app/svelte/lib/store/transcript.svelte.ts:48`), `Core::workspaces()`/`sessions_len()`/`pipe_counters()`/`config_for_cwd()`/`lane_to_protocol`/`OpenFacts` — these symbols DO NOT EXIST in the current tree (0 hits; stale earlier reads).
- Test evidence for load-bearing behavior kept: `tests_archive.rs` (archive-as-file-op, ADR-0005), `tests_turns.rs`/`tests_turn_guards.rs` (settlement + `TurnSettleGuard`, #267), `tests_dispatch.rs:53` (file_read paging), `tests_model.rs:114` (the "is not open" message), `tests_git.rs` (git snapshot/annotate).

## Out of scope notes

- `cargo doc` warnings at `crates/tau-core/src/tools/progress.rs:26, 117` (unresolved `[report]` links).
- The `tests_*`/`testkit.rs` suite (~8,300 LOC, files up to 837) is a separate wave; several B findings here are informed by test duplication (`tests_archive_edges.rs` carries two `too_many_lines` allows).
- `workspace_config` cache invalidation (C5) is a config-domain decision, not harness-runtime.

## Keep as-is

- `TurnSettleGuard` (`turn.rs:21-31`): clears `live.turn` on every exit path including panic, via `Drop` + `RefUnwindSafe` (#267). Model of a correct settlement invariant.
- `pipe_send` drop counters (`state.rs`): overflow summary emitted on next successful send — lossy-stream design done right.
- ADR-0005 archive path (`sessions_archive.rs`): archive is a pure file op off the live path; the 5s/child quiesce with re-insert-on-abort is the right shape (the repetition is B5, the design is not).
- `ForwardingProvider` + `ForwardSink` (`forwarding.rs`): the mirror-not-wrap provider with an explicit kill point is the cleanest seam in the harness.
- The `GitSnapshot` cache idea (`dispatch.rs:65-77`, single gix pass replacing five spawns per ADR-0011) — the caching discipline is right; only its invalidation (A7) and untracked numstat (A1) are wrong.
