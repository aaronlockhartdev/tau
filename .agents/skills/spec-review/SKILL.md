---
name: spec-review
description: Independent quality gate for tau spec docs — duplication, budgets, ID integrity, "Why:" relevance, traceability, prose, index accuracy. Use when reviewing staged doc changes, before committing spec/ or changes/, when a tasks.md line says "spec-review", or when asked to review documentation.
---

# Spec review

The reviewer ticks the `spec-review` task, never the implementer.
Judge the corpus, not the file: a rule that reads well and duplicates a rule elsewhere is a defect.

## Gate

Run the doc gates in `AGENTS.md` (spec tool, spec lint, doc lint, links) and follow its skipped/pass rule.

## Checklist

Scopes: **B** = behaviour changes only (skip with `N/A: docs-only`), **D** = docs/spec content, **A** = always.

| # | S | Check | Fail → |
|---|---|---|---|
| 1 | D | `grep -ril "<key phrase>" spec changes tests` finds no prior owner | duplicate: edit the owner, delete the copy |
| 2 | D | New requirement: ID from `tools/spec id`, one EARS clause, `Acceptance: @…` | block |
| 3 | B | Cited `@R-…` exists in `tests/features/`; scenario uses only steps in `STEPS.md` | block |
| 4 | D | No Given/When/Then text inside any spec | block |
| 5 | D | ID integrity per spec-change §3 (MODIFIED / REMOVED+Supersedes, no rename, renumber, reuse) | block |
| 6 | D | Delta paths mirror `spec/…` paths | fix |
| 7 | D | Each `Why:` passes "changes what code gets written?" and is ≤25 words | delete the line |
| 8 | D | No research provenance, benchmarks, citations, vendor news, or dates in `spec/` | move to archive |
| 9 | D | Files within the budgets in `AGENTS.md`; `spec/index.md` counts, links, and child lines updated | fix |
| 10 | D | Terms come from `docs/glossary.md`, not defined inline | move |
| 11 | D | Proposal names evidence per option, states the cost, marks unverified claims | block |
| 12 | D | Prose passes the ban list in spec-authoring | rewrite |
| 13 | A | `README.md` gained nothing internal (architecture, internal commands, doc map) | revert |
| 14 | A | No new dependency from core to a frontend; no CI step runs a real-LLM eval | block |
| 15 | A | Gates that exist are green; `M0` gates reported as skipped, not passed | block |

## Report

```text
verdict: approve | block
blockers: <check #, file:line, one line each>
notes: <non-blocking, ≤3>
```

Blockers are fixed before commit; notes go to the user. A blocked change stays open. A change that no longer matches reality is closed with a one-line reason in `proposal.md`.
