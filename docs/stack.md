# Stack

Decided, with pins; a deviation needs a change. A pin is a fact, so it lives here; the obligation to pin lives in `spec/workspace/spec.md`.

| Layer | Choice | Pin |
|---|---|---|
| Crates | `tau-core` · `tau-protocol` · `tau-acp` · `tau-app` · `tau-test` (mock LLM, harness, eval) · `tau-tools` (spec CLI) | workspace |
| Toolchain | Rust stable, latest release, bumped on release; `rust-toolchain.toml` pins it, `resolver = "3"` resolves MSRV-aware | 1.100.0 (2026-11-12) |
| UI | `gpui-kit`: `gpui-component` styled layer over `gpui-base`, in `crates/tau-app` only | gpui-kit 0.9 · MSRV Rust 1.92 |
| Async | tokio | latest stable |
| Behaviour tests | `cucumber`, `gherkin` (`harness = false`) | 0.23.0 · 0.16.0 |
| Unit + surface tests | `cargo nextest`, `#[gpui::test]` | — |
| Doc gates | `rumdl`, `lychee` | 0.3.4 · 0.24.2 |
| Markdown / diff | core parses with `pulldown-cmark`; the app renders it | spike in M0 |
| Config | TOML + hot reload (`add_user_config`) | `toml_edit` 0.25 · `notify` 8.2 |
| Storage | SQLite via `rusqlite` (`bundled`, WAL): append-only `events` + state tables, JSONL by export | rusqlite 0.41 · migrations 2.7 |
