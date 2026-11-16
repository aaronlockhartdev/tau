# Stack

Decided, with pins; a deviation needs a change. A pin is a fact, so it lives here; the obligation to pin lives in R-workspace-05, pending in `changes/change_spec_requirements`.

| Layer | Choice | Pin |
|---|---|---|
| Crates | `tau-core` · `tau-protocol` · `tau-acp` · `tau-app` · `tau-test` (harness, lane; a stub provider and an eval lane come with the loop) · `tau-tools` (the `spec` and `gates` CLIs) | workspace |
| Toolchain | Rust stable, latest release, bumped on release; `rust-toolchain.toml` pins it, `resolver = "3"` resolves MSRV-aware | 1.100.0 (2026-11-12) |
| UI | `gpui-kit`: `gpui-component` styled layer over `gpui-base`, to sit in `crates/tau-app` only | unpinned; lands with `spike_gpui_surface` and `add_surface_components` |
| Async | runtime rule: R-workspace-14, R-workspace-15 | tokio 1 (`macros`, `rt-multi-thread`), a `crates/tau-test` dev-dependency |
| Behaviour tests | `cucumber`, `gherkin` (`harness = false`) | 0.23.0 · 0.16.0 |
| Unit + surface tests | `cargo nextest` (`-P ci` in CI: `.config/nextest.toml`), `proptest` | nextest 0.9.152 · proptest 1.12 · `#[gpui::test]` lands with the surface, unpinned |
| Advisory gates | `cargo-deny` reads RustSec; Dependabot reads GitHub's reviewed advisories; the two drifted apart in 2026, so both run | cargo-deny 0.20.2 |
| CI | GitHub Actions lane: `.github/workflows/gates.yml` runs `tau-tools --bin gates` | actions pinned by commit SHA |
| Doc gates | `rumdl`, `lychee` | 0.3.4 · 0.24.2 |
| Markdown / diff | core is to parse with `pulldown-cmark`; the app renders it | unpinned; spike in M0 |
| Config | TOML + hot reload (`add_user_config`) | unpinned until that change lands; `toml_edit` is transitive at 0.25.19 today, `notify` absent |
| Storage | SQLite via `rusqlite` (`bundled`, WAL): the append-only `blocks` log, one row per block, and `requests`, one row per round-trip with its usage and its terminal; a block's state is a pair of pointers into the log; FTS5 index over the log, JSONL by export (`spec/session/storage/spec.md` at `adopt_block_log`'s archive: 01..06, 08, 10, 11, 13, 18, 28, 29, 31..40, 45, 46 — 07 and 30 retired, 41..44 pending in `add_turn_loop`); WAL needs a filesystem with working POSIX locks and shared memory, so a network mount is not a store host | rusqlite 0.41 (its `libsqlite3-sys` vendors SQLite 3.54.1) · migrations 2.7 · serde_json 1.0 |
