# Delta: harness

Apply to `spec/harness/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-harness-01  Acceptance refs resolve

WHEN the spec linter runs THE SYSTEM SHALL resolve every acceptance ref: a `@R-<domain>-<NN>` tag against a scenario in `crates/tau-test/tests/features/`, a `gate <name>` against a row in `AGENTS.md`, and a `spec-review check <n>` against a check in `.agents/skills/spec-review/SKILL.md`.
Acceptance: gate spec lint
Why: an unresolvable ref is a rule nobody enforces, and the ref is the traceability, not the clause.

### R-harness-02  Requirement IDs are allocated

WHEN the spec linter runs THE SYSTEM SHALL reject a requirement ID that `spec -- id <domain>` did not issue.
Acceptance: gate spec lint
Why: an ID nobody can trace leaves the requirement with no owner and no test.

### R-harness-03  Step vocabulary is generated

WHEN a step definition changes THE SYSTEM SHALL keep `crates/tau-test/tests/features/STEPS.md` matching the definitions in `crates/tau-test/src/steps/`.
Acceptance: gate steps current
Why: a hand-listed step drifts from the code, and a scenario then asserts something the runner never defined.

### R-harness-11  A withdrawn clause keeps its number

WHEN a delta clause sits under `## REMOVED Requirements` with no `Supersedes:` line, and `spec/` holds no clause with its id, THE SYSTEM SHALL keep that number consumed, require no acceptance ref for the clause, and leave `spec archive` no clause to delete.
Acceptance: gate spec lint
Why: an unissued hole reads as a hand-picked id, so dropping one rule costs renumbering every later id in the domain.
