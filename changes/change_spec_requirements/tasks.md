# Tasks: change_spec_requirements

- [x] audit the corpus against the requirement-focused standard
- [x] index invariants name their proof; mechanics and duplication cut
- [x] gates: `core has no UI logic`, `contracts current`, `live output under load`, `steps current`
- [x] skills: check 11b, `Why:` enumeration, index edited in place
- [x] templates: delta clause, step definition, `Why:` under ADDED
- [x] `workspace` domain row and stub
- [x] gate rows exist: `workspace layout` (manual) · `no local state tracked` · `toolchain pinned` (both `run`)
- [x] linter obligations land as R-harness-01..03 in the harness delta
- [x] R-harness-01,02: acceptance refs resolve in crates/tau-tools/src/bin/spec.rs (`gate_names`, `feature_tags`, `check_acceptance`): a tag names a feature scenario, a gate names an `AGENTS.md` row
- [x] R-harness-03: `crates/tau-test` bin `steps` generates crates/tau-test/tests/features/STEPS.md from the step definitions; the `steps current` gate compares them
- [x] R-harness-11: a withdrawal inside its own change keeps its number and needs nothing at archive (crates/tau-tools/src/bin/spec.rs, `## REMOVED Requirements`)
- [x] check: spec --check · contracts --check · steps --check · doc lint · links
- [ ] archive: after `add_turn_loop`, whose R-agent-15 scenario carries `@R-workspace-08`
- [x] Implement: `<project>/.tau/.gitignore` containing `*`, written once, and the store path inside `.tau` (`crates/tau-core/src/project.rs`; scenarios `@R-workspace-06,07` in crates/tau-test/tests/features/workspace.feature, `@R-session-18` in crates/tau-test/tests/features/session.feature)
- [ ] R-workspace-08: `Project::disclosure()` returns the line naming the write, and the scenario that prints it
is `add_turn_loop`'s `R-agent-15`, tagged `@R-workspace-08`
- [ ] docs: the delta-format sweep `adopt_block_log` carried here — bare `## ADDED` markers to
`## ADDED Requirements`, headings over the six-word cap, `THE SYSTEM SHALL NOT` obligations outside
the five EARS forms, the `Apply to` line naming each delta's live leaf, and the `IDs:` line ten of the
twelve open changes lack (`add_subagent_support`,
`add_surface_components`, `add_test_lanes`, `adopt_context_file_loading`, `adopt_hash_anchor_editing`,
`adopt_skills_convention`)
- [ ] docs: the clause-layout sweep `adopt_block_log` carried here — obligation, `Acceptance:` and
`Why:` adjacent, no blank line between them; the count and the per-change split are in
`changes/archive/2026-11-16-adopt_block_log/tasks.md`
- [ ] spec-review (reviewer ticks)
