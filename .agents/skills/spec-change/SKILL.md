---
name: spec-change
description: Run tau's change workflow: proposal with researched options, delta specs (ADDED/MODIFIED/REMOVED), tasks, apply, archive. Use when starting or continuing a behaviour change, adding a feature, deciding whether a feature is worth building, or archiving a finished change.
---

# Change workflow

Code never moves before its spec. Proposal → deltas → tasks → apply → archive.
A closed change in `changes/archive/` is the record of *why*; there is no separate decision log.

## 1. Open

`tools/spec new <verb>_<noun>` copies `changes/_template/` → `changes/<id>/`.
Verb carries intent: `add_`, `change_`, `remove_`, `adopt_`, `spike_`, `change_domains_`.

## 2. Proposal (`proposal.md`, ≤100 lines)

- **Context**: the problem in 3–6 sentences; existing behaviour by `R-ID`.
- **Options**: 2–4, each one line + evidence + cost. Evidence = measured in this repo (a number, a spike, a repro) or an external source with a link and a date. "Feels idiomatic", "widely used" is not evidence; mark anything unverified `unverified` and it cannot carry the decision. A mirror decision keeps the same shape: the options are mirror the reference and design it in-house, and the evidence names the reference's version and the section read.
- **Decision + consequences**: the option, what gets easier, what you now own (invariants, budgets, migrations, deletions).

State the cost: lines added to the corpus, tokens per turn, new dependencies. A feature that can't state its cost is not accepted.
If the value depends on real-LLM quality, name the eval suite in the proposal and run it by hand (invariant in `spec/index.md`).

## 3. Deltas

Delta files mirror the target: `changes/<id>/specs/<domain>/spec.md` for a domain index, `changes/<id>/specs/<domain>/<area>/spec.md` for a leaf.

```markdown
## ADDED Requirements
### R-<DOMAIN>-<NN>  <name>
WHEN <trigger> THE SYSTEM SHALL <response>.
Acceptance: @R-<DOMAIN>-<NN>

## MODIFIED Requirements
### R-<DOMAIN>-<NN>  <name>
WHEN <trigger> THE SYSTEM SHALL <new response>.
Acceptance: @R-<DOMAIN>-<NN>

## REMOVED Requirements
### R-<DOMAIN>-<NN>  <name>
Supersedes: R-<DOMAIN>-<NN> → R-<DOMAIN>-<MM>
```

- Describe the difference, never the whole file.
- Behaviour change = `MODIFIED` with full replacement text. Retirement = `REMOVED` + `Supersedes:`. IDs are never renamed, renumbered, or reused.
- Markers, one each: a delta clause that blocks coding carries `[NEEDS CLARIFICATION]`; an unresolved question in a proposal goes on the `Open:` line. `tau-spec --check` counts `[NEEDS CLARIFICATION]` and the count must not rise across a change.

## 4. Tasks (`tasks.md`, ≤15 lines)

One line per committable unit, ≤15 words each; implementation lines name an `R-ID`; spikes and gates first.

```markdown
- [ ] spike: cold build of {dep}, gate {n}s
- [ ] R-<DOMAIN>-<NN>: implement resume ({crate/src/path})
- [ ] R-<DOMAIN>-<NN>: scenario in tests/features/{domain}.feature
- [ ] spec: update spec/{domain}/…, bump spec/index.md
- [ ] spec-review (reviewer ticks)
```

Spec text moves in the same commit as the code it describes.

## 5. Apply

Run the gates in `AGENTS.md`. Gates that exist must be green; gates marked `M0` are reported in the commit body as skipped.

## 6. Archive

`tools/spec archive <id>` applies the deltas into `spec/` and moves the change to `changes/archive/<YYYY-MM-DD>-<id>/`.
Archive is append-only: never edited afterwards. New facts are a new change.
