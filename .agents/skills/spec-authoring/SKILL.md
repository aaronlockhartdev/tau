---
name: spec-authoring
description: Write or edit tau requirement specs under spec/: EARS clauses, R-IDs, where content belongs, acceptance refs, prose rules, and the relevance test for "Why:" notes. Use when adding a requirement, changing specified behaviour, editing spec/*.md, indexing a domain, or writing acceptance scenarios.
---

# Spec authoring

`spec/` = today's truth. `changes/` = proposals. `changes/archive/` = why. Budgets and gates: `AGENTS.md`.

## Where content goes

| Content | Goes in | Never in |
|---|---|---|
| A rule the system must follow | leaf `spec/<domain>/<area>/spec.md` | README, code comments, AGENTS.md (link, don't restate) |
| Observable behaviour, examples | `crates/tau-test/tests/features/<domain>.feature` tagged `@R-…` | spec; the spec cites the tag |
| Why a rule exists, the chosen option | `changes/<id>/proposal.md`, then archive | spec `Why:` (one line) |
| Term definition | `docs/glossary.md` | inline in any spec |
| Wire/tool shape | Rust types → generated `contracts/` | hand-written schema prose |
| Domain invariants, child index | `spec/<domain>/spec.md` | repeating a leaf rule |
| Cross-cutting invariant | `spec/index.md` | repeating it in a domain |

Over budget → split into children and add index lines. Don't compress prose until it's unreadable.

Every line in a leaf spec is an obligation, an acceptance ref, a reference to a `docs/glossary.md` term, or a one-line `Why:`. An invariant in `spec/index.md` or a domain index names the gate, lint, lane, or review check that proves it; a rule with no proof command is a leaf requirement with an `R-` ID.

## Requirement form (EARS)

One clause = one testable obligation:

- `WHEN <trigger> THE SYSTEM SHALL <response>`: event
- `WHILE <state> THE SYSTEM SHALL <response>`: state
- `WHERE <feature present> THE SYSTEM SHALL <response>`: context
- `IF <condition> THEN THE SYSTEM SHALL <response>`: exception
- `THE SYSTEM SHALL <response>`: invariant

```markdown
### R-<DOMAIN>-<NN>  <name ≤6 words>
WHEN <trigger> THE SYSTEM SHALL <response>.
Acceptance: @R-<DOMAIN>-<NN>
```

- Allocate the number with `cargo run -p tau-tools --bin spec -- id <domain>`. `<DOMAIN>` is the domain directory name verbatim (so `domain-model` gives `R-domain-model-01`); `<NN>` is digits, so the last hyphen separates them; both parts stay lowercase, exactly as `spec --id` prints them. Never hand-pick, reuse, renumber, or rename.
- One `###` per clause. "shall A and B" splits when A and B need separate tests.
- No `SHOULD` / `MAY` / "as much as possible": bound it or write no rule.
- Replace vague words with a number or a name: fast, large, minimal, appropriate, graceful, robust, seamless, user-friendly.

## Acceptance

Scenarios live in `crates/tau-test/tests/features/<domain>.feature`, tagged with the requirement's ID: `Acceptance: @R-<DOMAIN>-<NN>`.
A rule about a build fact the lanes cannot observe takes a gate instead: `Acceptance: gate <name>`, naming a row in `AGENTS.md`. A gate never replaces a scenario for observable behaviour.
Use only steps listed in `crates/tau-test/tests/features/STEPS.md`.
`Background` for shared preconditions, `Scenario Outline` + `Examples` for matrices. The spec never contains Given/When/Then.

## Why: notes

One line, ≤25 words, and only when an implementer reading the rule would otherwise do the opposite.
Test: *does this sentence change what code gets written?* If not, delete it.

Keep: a constraint the rule can't express (hardware, wire compatibility, a hard budget), a deliberate surprise.
Cut: how the decision was reached, benchmark numbers, citations, what competitors do, dates, and any
account of an earlier implementation; the record lives in `changes/archive/`.

## Prose

Fragments are fine. Tables beat sentences. One sentence beats a paragraph. In `spec/`, rationale is a `Why:` line, never a paragraph.
Banned: "this document describes", "it is important to note", "ensure that", leverage/robust/seamless, marketing voice, tutorials (README's job), commentary restating the EARS text.

## Before saving

1. `grep -ril "<key phrase>" spec changes tests`: if it hits, edit the owner, don't add a copy.
2. New requirement: allocated ID, EARS clause, `Acceptance:` tag, index line in the parent `spec.md`.
3. Each edited file within budget; update the counts in `spec/index.md`.
4. Run the gates in `AGENTS.md`.
5. New terms go in `docs/glossary.md`.
