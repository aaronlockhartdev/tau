# Tasks: adopt_quality_gates

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [x] spike: cold `cargo deny check` on this lockfile (gate: 10s), and `cargo clippy --workspace --all-targets --all-features -- -D warnings` with the deny-set (gate: clean on the scaffold)
- [x] R-workspace-09: delta clause in changes/adopt_quality_gates/specs/workspace/spec.md, then the deny-set in the root `Cargo.toml` `[workspace.lints.clippy]`; archive applies the clause
- [x] R-workspace-10: `lints` gate row gains `--all-features` (AGENTS.md)
- [x] R-workspace-11: `deny.toml`; `.github/dependabot.yml`; gate row `dependencies current`
- [x] R-workspace-11: the six crates gain `publish = false` (scaffold: 0.0.0, no `license`); `deny.toml` skips the license check for private members, so a crate that becomes publishable needs a `license` field first
- [x] R-workspace-12: `.github/workflows/gates.yml`; `.github/gate-coverage.sh`; gate row `ci covers gates`
- [x] R-workspace-13: `.config/nextest.toml` `[profile.ci]`; `unit + surface` gate row gains `-P ci`
- [x] docs: pins in `docs/stack.md`, layout rows in `spec/index.md`
- [x] check: nextest · features · spec --check · contracts --check · steps --check · clippy · fmt
- [x] spec-review (reviewer ticks)
