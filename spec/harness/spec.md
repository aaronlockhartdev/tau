# harness index

Owns: crates/tau-test, crates/tau-tools. Leaves: none. Requirements: 1.

## Requirements

### R-harness-10  Generated inputs check the clause scans

WHEN the unit lane runs THE SYSTEM SHALL check the spec tool's clause scans on generated markdown: `clauses_in` and `clause_range` agree on every clause ID, a `REMOVED` delta leaves every other clause intact, and blank-run collapse is idempotent.
Acceptance: gate unit + surface
