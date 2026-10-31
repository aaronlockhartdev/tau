# Hygiene audit rubric

Shared instructions for the audit agents. Findings are evidence for a cleanup wave, not
a style report. Every finding is a file:line, a claim, and a cost.

## Axes

1. **Dead code.** Unreachable public API; `pub` items with no caller in the workspace;
   unused fields/variants/flags; test-only helpers compiled into production; abstractions
   with one implementation that adds no seam value; config knobs nobody sets; defensive
   branches the type system already excludes. Verify a "no caller" claim with `rg` across
   `crates/`, `app/src-tauri/`, `app/svelte/`, `app/tests/`, `fuzz/`, `eval/` before writing it.
2. **Simplify.** Duplication that is one module's job; wrappers that only forward; N-step
   call chains where 2 do; parallel models of the same concept (two representations, two
   stores, two encoders); speculatively general code (generics/features for one caller);
   comment noise that AGENTS.md forbids; files drifting toward the 500-LOC soft limit with
   more than one reason to change; panics/unwrap on a foreseeable path.
3. **Offload.** Hand-rolled logic a well-maintained third-party crate already owns
   (Rust: crates.io; Svelte/TS: npm). For each: what we hand-roll, LOC, the crate that
   owns it, and the migration risk. Only propose a crate that is actively maintained,
   MIT/Apache-2.0/BSD/ISC-compatible with AGPL-3.0-only, and already adjacent to
   dependencies we ship. Fewer deps is a virtue; "we hand-roll this" is a finding only
   when the wheel is real and heavy (>50 LOC of tricky logic, or a correctness risk).

## Rules for the report

- Scope is exactly what the brief names. Do not audit outside it; note cross-scope
  observations in one line under **Out of scope notes**.
- Cite `path:line` for every finding. State LOC.
- Severity: `A` = dead weight or real risk, do now; `B` = simplification worth a ticket;
  `C` = smell, batch with the next touch.
- "No finding" is a valid, useful result — say so plainly per axis rather than inventing filler.
- Do not edit any file except your own findings doc. Read-only otherwise.
- Check the mechanical gates yourself where cheap: `cargo clippy -p <crate> --all-targets`,
  `cargo doc -p <crate>`, `npx eslint`, `npx vitest run <path>`. Report new warnings; do not fix.
- Name the test evidence: for each behavior you claim is load-bearing, cite the test file.

## Findings doc format

```
# <scope> — hygiene audit (2026-10-28)
Scope: <paths, LOC counted by wc -l>
Baseline: <gate commands run, pass/fail, warning counts>

## A — do now
## B — ticket
## C — smell
## Offload candidates
## Dead-code evidence
<the rg probes that proved each "no caller" claim, so the claim is re-checkable>
## Out of scope notes
## Keep as-is
<the 3-5 things in scope that are the model of how to do it right>
```
