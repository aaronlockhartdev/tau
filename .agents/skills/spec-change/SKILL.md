---
name: spec-change
description: Run tau's change workflow: proposal with researched options, delta specs (ADDED/MODIFIED/REMOVED), tasks, apply, archive. Use when starting or continuing a behaviour change, adding a feature, deciding whether a feature is worth building, or archiving a finished change.
---

# Change workflow

Code never moves before its spec. Proposal → deltas → tasks → apply → archive.
A closed change in `changes/archive/` is the record of *why*; there is no separate decision log.

## 1. Open

`cargo run -p tau-tools --bin spec -- new <verb>_<noun>` copies `changes/_template/` → `changes/<id>/`.
Verb carries intent: `add_`, `change_`, `remove_`, `adopt_`, `spike_`, `scaffold_`, `change_domains_`.

## 2. Proposal (`proposal.md`)

- **Context**: the problem in 3–6 sentences; existing behaviour by `R-ID`.
- **Options**: 2–4, each one line + evidence + cost. Evidence = measured in this repo (a number, a spike, a repro) or an external source with a link and a date. "Feels idiomatic", "widely used" is not evidence; mark anything unverified `unverified` and it cannot carry the decision. A mirror decision keeps the same shape: the options are mirror the reference and design it in-house, and the evidence names the reference's version and the section read.
- **Decision + consequences**: the option, what gets easier, what you now own (invariants, migrations, deletions), and the leaf spec each new rule lands in. A rule you own and do not write as a delta clause never enters `spec/`.

- **Eval**: `none needed`, or the suite name, when the value depends on real-LLM quality.

State the cost: new dependencies, runtime cost, and tokens per turn where a turn pays it. A feature that can't state its cost is not accepted.
If the value depends on real-LLM quality, name the eval suite in the proposal and run it by hand (invariant in `spec/index.md`).

## 3. Deltas

`spec --new` copies `changes/_template/specs/_domain_/[<area>/]spec.md`; rename `_domain_` (and `_area_`) to the target's directory names, so the delta path mirrors the target. `changes/<id>/specs/<domain>/spec.md` is a domain index, `changes/<id>/specs/<domain>/<area>/spec.md` for a leaf. A domain with no children keeps its rules in `spec/<domain>/spec.md`, and the delta sits at the same path. A domain index gains an invariant only through the change that names its proof. `spec/index.md` is edited in place by the change that touches it: its invariants name a proof and carry no `R-` ID.

```markdown
## ADDED Requirements
### R-<domain>-<NN>  <name>
WHEN <trigger> THE SYSTEM SHALL <response>.
Acceptance: @R-<domain>-<NN>

### R-<domain>-<NN>  <name> (build fact the lanes cannot observe)
THE SYSTEM SHALL <response>.
Acceptance: gate <name in AGENTS.md>

## MODIFIED Requirements
### R-<domain>-<NN>  <name>
WHEN <trigger> THE SYSTEM SHALL <new response>.
Acceptance: @R-<domain>-<NN>

## REMOVED Requirements
### R-<domain>-<NN>  <name>
Supersedes: R-<domain>-<NN> → R-<domain>-<MM>
```

- Describe the difference, never the whole file.
- Behaviour change = `MODIFIED` with full replacement text. Retirement = `REMOVED` + `Supersedes:`. A clause keeps its ID from the first scenario citation: never renamed, renumbered, or reused. A delta clause no scenario has run yet may be renumbered inside its own change.
- Markers, one each: a delta clause that blocks coding carries `[NEEDS CLARIFICATION]`; an unresolved question in a proposal goes on the `Open:` line. `cargo run -p tau-tools --bin spec -- --check` counts `[NEEDS CLARIFICATION]` and the count must not rise across a change.

## 4. Tasks (`tasks.md`)

One line per committable unit, phrased as an action; implementation lines name an `R-ID`; spikes and gates first.

```markdown
- [ ] spike: cold build of {dep}, gate {n}s
- [ ] R-<domain>-<NN>: delta clause in changes/{id}/specs/{domain}/…, then implement resume ({crate/src/path})
- [ ] R-<domain>-<NN>: scenario in crates/tau-test/tests/features/{domain}.feature
- [ ] R-<domain>-<NN>: step {name} in crates/tau-test/src/steps/ (STEPS.md regenerates)
- [ ] check: nextest · features · spec --check · contracts --check · steps --check · clippy · fmt
- [ ] spec-review (reviewer ticks)
```

No code lands before its delta clause exists; a docs-only delta lands alone while implementation is blocked (for tau, until `scaffold_workspace`). `archive` moves the deltas into `spec/` at close.

## 5. Apply

Run the gates in `AGENTS.md`. Gates that exist must be green; gates marked `M0` are reported in the commit body as skipped.

A decision worth keeping past the session lands in `spec/` through a change; prompt state is not a record.

## 6. Archive

`archive` needs each clause's acceptance ref to have run: a scenario that has never run, or a gate that has never passed, keeps the change open.
`cargo run -p tau-tools --bin spec -- archive <id>` applies the deltas into `spec/` and moves the change to `changes/archive/<YYYY-MM-DD>-<id>/`.
Archive creates `spec/<domain>/spec.md` when the delta names a domain that has no file yet. Archive is append-only: never edited afterwards. New facts are a new change.
