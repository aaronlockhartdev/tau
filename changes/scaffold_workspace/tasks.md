# Tasks: scaffold_workspace

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [x] Gate rows: flip every `AGENTS.md` row whose command the scaffold gives from `M0` to `run` (the list, and what each row does not yet prove, is in the proposal's Consequences), and `spec/index.md` drops the count's "manual until M0"
- [x] R-workspace-01: root `Cargo.toml` as a virtual manifest with `members = ["crates/*"]` and `resolver = "3"`; `[workspace.lints]` and `[workspace.dependencies]` at the root
- [x] R-workspace-05: `rust-toolchain.toml` pinning the stable release named in `docs/stack.md`
- [x] R-workspace-02: the six members of `add_repo_layout`, each named after its directory with `[lints] workspace = true`: `tau-core`, `tau-protocol`, `tau-acp`, `tau-app`, `tau-test`, `tau-tools`
- [x] R-workspace-04: `tau-protocol` bin `contracts` writing the generated schemas into `contracts/`, `--check` comparing the generated set against the directory
- [x] R-harness-01,02: `tau-tools` bin `spec` subcommands `id`, `new`, `count`, `archive`, and `--check` resolving every acceptance ref (`@R-…` against the feature tree, `gate …` against `AGENTS.md`), rejecting an ID `id` did not issue, and taking the skills' `--id`/`--new` shorthand
- [x] R-harness-02: `--check` reports a number `id` never issued — every number up to a domain's highest is live in the corpus or archived (a retired clause leaves `spec/` by design), and one in neither is unissued
- [x] R-harness-01,02: `--check` skips the `_template`, `_domain_` and `_area_` placeholder copies `spec new` writes, so a freshly opened change lints before it is edited
- [x] R-harness-03: `tau-test` bin `steps` generating `crates/tau-test/tests/features/STEPS.md` from the definitions in `crates/tau-test/src/steps/`, `--check` comparing the two
- [x] R-harness-04: cucumber World and step definitions in `crates/tau-test/src/`, the `features` test target with `harness = false`, and a stub feature asserting the R-workspace-01..03 layout facts — this is the measurement of the cucumber-rs path claim in `add_repo_layout`
- [x] check: spec id/new/count/archive · nextest · features · spec --check · contracts --check · steps --check · doc lint · links · clippy · fmt · cargo metadata · git ls-files · cargo tree · rust-toolchain.toml against `docs/stack.md`; the commit body reports the `M0` rows as `skipped (M0)` and the eval row as `manual`
- [ ] spec-review (reviewer ticks)
