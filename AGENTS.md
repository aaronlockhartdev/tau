# tau: agent guide

Agent-facing. `README.md` is user-facing; nothing here belongs there.
Auto-loaded: this file (nested `AGENTS.md` files, nearest wins) and each skill's name + description. Everything else is read by path, on demand.

## Read order for a task

1. This file.
2. `spec/index.md`: cross-cutting invariants, then the 1–2 domains the task touches.
3. That domain's `spec.md`, then only the leaf `spec.md` you need.
4. Changing behaviour? `changes/<id>/` if one exists; if not, open one (skill: spec-change).
5. A skill, when the task matches its description.

Read nothing else by default. If the load feels heavy, you are reading files you don't need.

## Layout

| Path | What | Read when |
|---|---|---|
| `spec/index.md` | cross-cutting invariants, domain table, corpus count | any spec work |
| `spec/<domain>/spec.md` | area index: invariants, child links | entering a domain |
| `spec/<domain>/<area>/spec.md` | leaf: EARS requirements + acceptance refs | implementing that area |
| `changes/<id>/` | active change: proposal, deltas, tasks | before coding a behaviour change |
| `changes/archive/` | closed changes, the record of *why* | history of a rule |
| `tests/features/` | Gherkin acceptance, tagged `@R-…` | writing/reading acceptance |
| `tests/features/STEPS.md` | allowed step vocabulary | before writing a scenario |
| `contracts/` | generated wire/tool schemas | touching the wire |
| `docs/glossary.md` | canonical terms | a term is ambiguous |
| `.agents/skills/`, `.agents/agents/` | skills (below); subagent definitions | task matches |

## Gates

`run` = works today. `M0` = created by the M0 scaffold; report `skipped (M0)`, never fake a pass. A change merges when every `run` gate is green and the rest are reported skipped.

| Gate | Command | State |
|---|---|---|
| spec tool | `tools/spec {id,new,count,archive}` | M0 |
| spec lint | `cargo run -p tau-test --bin tau-spec -- --check` | M0 |
| doc lint | `rumdl lint spec changes docs AGENTS.md README.md` | M0 |
| links | `lychee spec changes docs AGENTS.md README.md` | M0 |
| unit + surface | `cargo nextest run` | M0 |
| behaviour | `cargo test --test features` | M0 |
| lints | `cargo fmt && cargo clippy -- -D warnings` | M0 |
| headless session | `cargo run -p tau-core --example run -- --tape <f>` | M0 |
| desktop | `cargo run -p tau-app` | M0 |
| eval (never CI) | `cargo run -p tau-test --bin eval -- --suite suites/<x>.md` | manual |

## Stack

Decided, with pins; deviations need a change.

| Layer | Choice | Pin |
|---|---|---|
| Crates | `tau-core` · `tau-protocol` · `tau-acp` · `tau-app` · `tau-test` · `tau-mock-llm` | workspace |
| Toolchain | Rust stable, latest release, bumped on release; `rust-toolchain.toml` pins it, `resolver = "3"` resolves MSRV-aware | 1.100.0 (2026-11-12) |
| UI | `gpui-kit`: `gpui-component` styled layer over `gpui-base`, in `crates/tau-app` only | gpui-kit 0.9 · MSRV Rust 1.92 |
| Async | tokio | latest stable |
| Behaviour tests | `cucumber`, `gherkin` (`harness = false`) | 0.23.0 · 0.16.0 |
| Unit + surface tests | `cargo nextest`, `#[gpui::test]` | — |
| Doc gates | `rumdl`, `lychee` | 0.3.4 · 0.24.2 |
| Markdown / diff | core parses with `pulldown-cmark`; the app renders with `gpui-kit`'s `TextView` and `Diff` | spike in M0 |
| Config | TOML + hot reload (`add_user_config`) | `toml_edit` 0.25 · `notify` 8.2 |
| Storage | SQLite via `rusqlite` (`bundled`, WAL): append-only `events` + state tables, JSONL by export | rusqlite 0.41 · migrations 2.7 |

## Budgets

| Item | Max |
|---|---|
| `AGENTS.md` | 120 lines |
| `spec/index.md` | 60 lines |
| domain index | 120 lines |
| leaf spec | 150 lines |
| proposal / tasks | 100 / 15 lines |
| `spec/` corpus | 40k tokens |

Authoring and review rules belong to the skills; don't restate them here.

## Working tree

Land on `main`: signed commits, one committable unit each, linear history (rebase, never a merge commit). Commit body names the change id and the gate status, including `skipped (M0)`.
A branch is only for work that can't land today (a spike with a build gate, a migration). Name it after the change id; delete it at land.
No worktrees by default. Trigger: two writer agents running at once, or a bisect/repro that needs a clean tree. Then one worktree per writer at `../tau-<id>` (never inside the repo), with `CARGO_TARGET_DIR=$HOME/.cache/tau-target` shared, so disk stays 1x and builds serialize. Cargo flags shared target dirs as unsafe upstream. Create and remove it in the same session; leave no uncommitted state; `git worktree prune` after any manual cleanup.

## Skills

- **spec-authoring**: write/edit requirements: EARS, IDs, where content goes, prose, `Why:`
- **spec-change**: proposal, deltas, tasks, apply, archive
- **spec-review**: gate before committing docs
