## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues, driven via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Coding conventions

Rules, not suggestions. Enforced in CI where mechanical, in review where not.

**Comments — anti-slop discipline.**
- A comment is justified only by a *why* that is not obvious from the code: a constraint, a workaround, a non-obvious invariant, a decision citation (spec §/ADR/ticket).
- Never narrate what the code does ("loads the config"), never restate the name in prose, never use banner or section-divider comments, never write "Adds/Updates X" change narration.
- No speculative TODOs — open the ticket instead. A TODO that names no ticket and no reason is noise.

**Naming over commenting.** If a name needs a comment to be understood, fix the name first.

**Formatting.** Rust: `rustfmt` defaults + `clippy -D warnings`, both enforced in CI. Svelte/TypeScript: same spirit — consistent 2-space indent, single quotes, semicolons on; no extra formatter tooling in v0.

**No speculative code.** No dead abstractions, no "for the future" scaffolding, no flags for nonexistent features, no second implementation kept "just in case". Build the thing the ticket asks for; the next ticket extends it.

**Doc comments** appear only on public API items whose contract is not self-evident from the signature — one line where possible, with a spec/ADR citation when a rule comes from one.
