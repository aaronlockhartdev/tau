# change_spec_requirements

## Context

`spec/` carries 5 cross-cutting invariants and 0 requirements while 11 open proposals specify behaviour in prose. An audit of the corpus (2026-11-13) found 14 obligation groups with no path into `spec/`, 13 rules an implementer would violate without a `Why:` note, and 5 spec lines that are table mechanics, self-duplication, or narration rather than obligations. `spec/index.md` demands an `R-<DOMAIN>-<NN>` ID and a scenario for every requirement, which the unnumbered invariants cannot satisfy, so the rule reads as waived for the index. Decisions are recorded; specified behaviour is not, and that gap is where an implementer guesses.

## Options

- A. Invariants cite their proof: each line in `spec/index.md` names the gate, lint, lane, or review check that proves it, every other rule becomes a leaf requirement with an `R-` ID and a delta clause, and `spec-review` blocks a proposal whose owned rules have no delta. Evidence: measured here — the audit's 14 findings were all proposals with no delta file, and `spec-review` checked clauses inside a delta while never asking whether a delta existed. Cost: no dependencies, no runtime; 6 gate rows, ~6 words per invariant on any turn that loads the index.
- B. Give the invariants `R-` IDs and scenarios. Evidence: measured here — the index's own second bullet reads this way. Cost: 5 scenarios with no runnable harness (M0), and the proof of "core carries no UI logic" is a `cargo tree` run, not a Gherkin scenario.
- C. Keep invariants as prose principles. Evidence: measured here — the status quo, in which 5 index lines already drifted into mechanics and duplication. Cost: every future index edit re-argues what belongs there.

## Decision

A: an invariant is a rule with a named proof, everything else a numbered requirement with a delta clause. An implementer reads obligations they can check, and a rule nobody can prove stops posing as a spec line.
Consequences: `spec/index.md` bullets name their gate, lint, lane, or review check, and the committed-marker clause moves to `add_session_storage`'s delta as R-session-07. A `workspace` domain takes the repo-layout rules as R-workspace-01..08 in `spec/workspace/spec.md`, the spec `add_repo_layout` now names as its destination. `spec-review` gains check 11b (owned rule, no delta → block); `spec-change` §3 says index files are edited in place and code lands with its delta clause; the templates list a delta clause, a step definition, and a `Why:` slot under ADDED. Two gates exist to make invariants checkable: `live output under load`, `steps current`. The obligations behind `spec lint` and `steps current` land as R-harness-01..03 in `spec/harness/spec.md`. Numerical file and corpus budgets are removed repo-wide: length is a judgement spec-review makes, `AGENTS.md` keeps commands and rules that bind every task, pins move to `docs/stack.md`, and an ID becomes permanent at its first scenario citation, which lets an unrun delta clause be renumbered inside its own change. The domain table and Layout table in `spec/index.md` name owned paths and what to read; neither restates a rule. See spec-authoring.
Eval: none needed.

Open: none.
