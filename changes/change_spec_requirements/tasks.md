# Tasks: change_spec_requirements

- [x] audit the corpus against the requirement-focused standard
- [x] index invariants name their proof; mechanics and duplication cut
- [x] gates: the four rows this change added — `core has no UI logic`, `contracts current`, `steps current`
PASS; `live output under load` reported `skipped (M0)`, its state, nothing implementing it. The report over
every row is the unticked `check:` line naming `--bin gates`
- [x] skills: check 11b, `Why:` enumeration, index edited in place
- [x] templates: delta clause, step definition, `Why:` under ADDED
- [x] `workspace` domain row and stub
- [x] gate rows exist: `workspace layout` (manual) · `no local state tracked` · `toolchain pinned` (both `run`)
- [x] linter obligations land as R-harness-01..03 in the harness delta
- [x] R-harness-01,02: acceptance refs resolve in crates/tau-tools/src/bin/spec.rs (`gate_names`, `feature_tags`, `check_acceptance`): a tag names a feature scenario, a gate names an `AGENTS.md` row
- [x] R-harness-03: `crates/tau-test` bin `steps` generates crates/tau-test/tests/features/STEPS.md from the step definitions; the `steps current` gate compares them
- [x] R-harness-11: a withdrawal inside its own change keeps its number and needs nothing at archive (crates/tau-tools/src/bin/spec.rs, `## REMOVED Requirements`)
- [x] check: spec --check · contracts --check · steps --check · doc lint · links
- [ ] archive: after `add_turn_loop`, whose R-agent-15 scenario carries `@R-workspace-08`. The deltas add 4
harness clauses and 8 workspace clauses, and the counts move from a stated source: `spec/harness/spec.md`'s
count is 1 today and no other open delta adds clauses to that index (add_test_lanes fills the `lanes` leaf),
so it reads 5 here; `spec/workspace/spec.md` is 7 today and `add_turn_loop` lands R-workspace-16..19 first,
so 11 → 19; `spec/index.md`'s Corpus is 33, and `add_turn_loop` lands 28 clauses first, so 61 → 73. Read each
off the file at archive. No Leaves cell moves: both deltas are domain indexes
- [x] Implement: `<project>/.tau/.gitignore` containing `*`, written once, and the store path inside `.tau` (`crates/tau-core/src/project.rs`; scenarios `@R-workspace-06,07` in crates/tau-test/tests/features/workspace.feature, `@R-session-18` in crates/tau-test/tests/features/session.feature)
- [x] docs: the six-word heading cap, swept across the open deltas: 17 ADDED headings re-worded
(`add_subagent_support` 9, `add_surface_components` 3, `add_user_config` 2, `add_test_lanes` 1,
`adopt_hash_anchor_editing` 1, `adopt_skills_convention` 1); the ids are listed at
`changes/archive/2026-11-16-adopt_block_log/tasks.md`, and no open delta heading runs over now
- [ ] docs: the delta-format sweep `adopt_block_log` carried here — bare `## ADDED` markers to
`## ADDED Requirements`, clauses phrased as a bare negation (no negated form in `spec-authoring`'s
§Requirement form), the `Apply to` line naming each delta's live leaf, and the `IDs:` line — ten of the
twelve open changes lack an `IDs:` line, `add_turn_loop` listing its claims under `IDs:` and
`add_realtime_gui_output` opening with an `IDs:` task; the task lines themselves name clause IDs in eleven
of the twelve. The nine delta files the sweep re-formats, in six
changes, are
`add_subagent_support`, `add_surface_components`, `add_test_lanes`, `adopt_context_file_loading`,
`adopt_hash_anchor_editing`, `adopt_skills_convention`
- [ ] docs: the clause-layout sweep `adopt_block_log` carried here — obligation, `Acceptance:` and
`Why:` adjacent, no blank line between them; the count and the per-change split are in
`changes/archive/2026-11-16-adopt_block_log/tasks.md`
- [ ] R-workspace-08: the scenario that prints the disclosure line in session start output; `Project::disclosure()`
returns it (`crates/tau-core/src/project.rs`) and `@R-workspace-08` is tagged, so the print lands with
`add_turn_loop`'s `R-agent-15`
- [ ] spec-review (reviewer ticks)
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, the manual row
`workspace layout` by hand
