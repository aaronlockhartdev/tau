# add_repo_layout

## Context

The corpus names paths without a tree: `crates/tau-core/storage`, `crates/tau-app`, `tests/features/`, `tools/spec`, `contracts/`, `.tau/state.db`. The planned `scaffold_workspace` change cannot start until the tree, the workspace manifest shape, and the directory each gate command runs from are settled. Six crates are already named in `AGENTS.md`, and `spec/index.md` names module directories inside `crates/tau-core/` in its `owns` column, so `tau-core` holds modules and not crates. Two constraints come from tooling: cargo's own guidance on member layout, and cucumber-rs, whose feature paths in the crate's docs start at the crate root (`/tests/features/...`), so a feature tree placed outside the package that runs it is not reachable by the documented call. The app crate carries the whole GPUI dependency weight, so the layout has to keep that crate separate and checkable.

## Options

- A. Virtual workspace at the root; every member under `crates/*` with the directory named after the crate; non-cargo content stays at the root. Evidence: the cargo book recommends keeping all members in a flat directory commonly `crates/` with `members = ["crates/*"]` ([doc.rust-lang.org/cargo/reference/workspaces.html](https://doc.rust-lang.org/cargo/reference/workspaces.html), read 2026-11-13); rust-analyzer's layout is a virtual manifest at the repo root with everything else nested one level under `crates/` and each directory named for its crate ([Large Rust Workspaces](https://matklad.github.io/2021-08-22/large-rust-workspaces.html), 2021-08-22, read 2026-11-13); Zed's repo is `crates/` plus `assets/`, `docs/`, `script/` at the root ([github.com/zed-industries/zed](https://github.com/zed-industries/zed), read 2026-11-13). Cost: 1 root manifest, 6 member directories.
- B. Members as top-level directories. Evidence: RFC 1525 shows root-level members in its workspace example ([rust-lang.github.io/rfcs/1525-cargo-workspace.html](https://rust-lang.github.io/rfcs/1525-cargo-workspace.html), read 2026-11-13). Cost: a members glob sweeps `spec/`, `docs/`, `changes/`, so the list becomes hand-maintained with `exclude` entries, and crate roots sit next to runtime state.
- C. One crate, modules only. Evidence: `spec/index.md` already treats spec areas as modules inside `tau-core`, and workspace inheritance covers shared metadata ([cargo book, workspace tables](https://doc.rust-lang.org/cargo/reference/workspaces.html), read 2026-11-13). Cost: 1 manifest, and nothing stops a `tau-app` import inside core, since a module cannot declare dependencies the way a package can. Ruled out.
- D. `apps/` and `libs/` split by kind. Evidence: no source read; rust-analyzer and Zed both use one flat `crates/` (both, read 2026-11-13). Cost: two naming conventions where one carries the same information. Ruled out.

## Decision

A: a virtual workspace whose members are `crates/*`, with the crate name equal to the directory name.

```text
Cargo.toml            [workspace] members = ["crates/*"], resolver = "3", [workspace.lints], [workspace.dependencies]
rust-toolchain.toml   latest stable, bumped on release
crates/tau-core/          lib: one module per spec domain (see the owns column in spec/index.md)
crates/tau-protocol/      lib: wire types; a `contracts` bin writes the JSON Schemas into contracts/
crates/tau-acp/           lib: the ACP adapter, the only user of agent-client-protocol
crates/tau-app/           bin: the gpui-kit desktop app, the only crate that sees gpui
crates/tau-test/          lib + [[test]] features: cucumber World and step defs; tests/features/ holds the .feature files; suites/ holds the eval suites; bin mock-llm scripts the LLM over TCP; bin eval runs suites by hand
crates/tau-tools/          bin spec: id, new, count, archive, --check on the spec corpus
spec/  changes/  docs/  contracts/  .agents/  .tau/
```

Consequences: the corpus moves two paths, `tools/spec` → `cargo run -p tau-tools --bin spec --` and `tests/features/` → `crates/tau-test/tests/features/` (gate: `cargo test -p tau-test --test features`), in `AGENTS.md`, both skills, `STEPS.md` and `spec/index.md`; the `AGENTS.md` Crates row gains `tau-tools`, and the harness `owns` row in `spec/index.md` gains the moved feature tree and `crates/tau-tools`; `spec/index.md`'s `owns` column names module directories inside `crates/tau-core/`, matching the unified package rule. The core-decoupling invariant gains a gate row in `AGENTS.md`. `.tau/` is runtime state and gitignored; `.agents/` is tracked. `resolver = "3"` and `[workspace.lints]` are set at the root, so members inherit the lint set and the MSRV-aware floor. cucumber-rs resolving `/tests/features/...` from the crate root is `unverified` until the first `scaffold_workspace` task runs a stub feature. No new dependency: `tau-tools` replaces a script, and no member depends on gpui except `tau-app`. Adopted 2026-11-13: the moves landed in `AGENTS.md`, both skills, `STEPS.md`, `spec/index.md`, `spec/harness/spec.md` and the tasks template, and `tau-mock-llm` folds into `crates/tau-test/` as `bin mock-llm`, so the workspace has six members.

Eval: none needed.

Open: none.
