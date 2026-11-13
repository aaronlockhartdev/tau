# Spec index

Cross-cutting invariants live here and are never restated in a domain:

- Core depends on no frontend. Frontends: `crates/tau-app` (GPUI), `crates/tau-acp`.
- Every requirement has an allocated `R-<DOMAIN>-<NN>` and ≥1 scenario in `crates/tau-test/tests/features/` tagged with it.
- Wire shapes live in Rust types; `contracts/` is generated, never hand-edited.
- Real-LLM eval is run by hand when deciding whether a feature is worth building; never in CI.
- Live output is independent of storage. Frontends consume core's in-memory event stream, rendering reads no storage, and no storage write, checkpoint, or reader gates emission. The committed marker is a storage fact, set by the transaction that commits a turn.

## Domains

One row per domain: read its `spec.md` before any leaf. Counts come from `cargo run -p tau-tools --bin spec -- count` (manual until M0).
Split a domain into children, or merge domains, through a change (`change_domains_*`). The `owns` column follows the crate layout, which moves.

| Domain | Owns | Spec | Leaves | Lines |
|---|---|---|---|---|
| domain-model | `crates/tau-core/model` | [domain-model/spec.md](domain-model/spec.md) | — | 2 |
| protocol | `crates/tau-protocol` | [protocol/spec.md](protocol/spec.md) | — | 2 |
| session | `crates/tau-core/session`, `crates/tau-core/storage` | [session/spec.md](session/spec.md) | — | 2 |
| context | `crates/tau-core/context` | [context/spec.md](context/spec.md) | — | 2 |
| tools | `crates/tau-core/tools` | [tools/spec.md](tools/spec.md) | — | 2 |
| acp | `crates/tau-acp` | [acp/spec.md](acp/spec.md) | — | 2 |
| surface | `crates/tau-app` | [surface/spec.md](surface/spec.md) | — | 2 |
| harness | `crates/tau-test`, `crates/tau-test/suites`, `crates/tau-tools` | [harness/spec.md](harness/spec.md) | — | 2 |
| config | `crates/tau-core/config` | [config/spec.md](config/spec.md) | — | 2 |

Corpus: 0 requirements, 0 / 40k tokens. Each requirement arrives through a change.
