# Spec index

Cross-cutting invariants live here and are never restated in a domain:

- Core depends on no frontend. Frontends: `crates/tau-app` (GPUI), `crates/tau-acp`.
- Every requirement has an allocated `R-<DOMAIN>-<NN>` and ≥1 scenario in `tests/features/` tagged with it.
- Wire shapes live in Rust types; `contracts/` is generated, never hand-edited.
- Real-LLM eval is run by hand when deciding whether a feature is worth building; never in CI.

## Domains

One row per domain: read its `spec.md` before any leaf. Counts come from `tools/spec count` (manual until M0).
Split a domain into children, or merge domains, through a change (`change_domains_*`). The `owns` column follows the crate layout, which moves.

| Domain | Owns | Spec | Leaves | Lines |
|---|---|---|---|---|
| domain-model | `crates/tau-core/model` | [domain-model/spec.md](domain-model/spec.md) | — | 3 |
| protocol | `crates/tau-protocol` | [protocol/spec.md](protocol/spec.md) | — | 3 |
| session | `crates/tau-core/session` | [session/spec.md](session/spec.md) | — | 3 |
| context | `crates/tau-core/context` | [context/spec.md](context/spec.md) | — | 3 |
| tools | `crates/tau-core/tools` | [tools/spec.md](tools/spec.md) | — | 3 |
| acp | `crates/tau-acp` | [acp/spec.md](acp/spec.md) | — | 3 |
| surface | `crates/tau-app` | [surface/spec.md](surface/spec.md) | — | 3 |
| harness | `crates/tau-test`, `crates/tau-mock-llm`, `tests/` | [harness/spec.md](harness/spec.md) | — | 3 |
| config | `crates/tau-core/config` | [config/spec.md](config/spec.md) | — | 3 |

Corpus: 0 requirements, 0 / 40k tokens. Each requirement arrives through a change.
