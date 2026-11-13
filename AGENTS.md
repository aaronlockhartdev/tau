# tau — agent guide

Agent-facing. `README.md` is user-facing; nothing here belongs there.
Auto-loaded: this file (nested `AGENTS.md` files, nearest wins) and each skill's name + description. Everything else is read by path, on demand.

## Read order for a task

1. This file.
2. `spec/index.md` — cross-cutting invariants, then the 1–2 domains the task touches.
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
| `changes/archive/` | closed changes — the record of *why* | history of a rule |
| `tests/features/` | Gherkin acceptance, tagged `@R-…` | writing/reading acceptance |
| `tests/features/STEPS.md` | allowed step vocabulary | before writing a scenario |
| `contracts/` | generated wire/tool schemas | touching the wire |
| `docs/glossary.md` | canonical terms | a term is ambiguous |
| `.agents/skills/` | skills below | task matches |

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

## Budgets

| Item | Max |
|---|---|
| `AGENTS.md` | 120 lines |
| `spec/index.md` | 60 lines |
| domain index | 120 lines |
| leaf spec | 150 lines |
| proposal / tasks | 100 / 15 lines |
| `spec/` corpus | 40k tokens |

Owner of authoring and review rules is the skill, not this file — don't restate them here.

## Skills

- **spec-authoring** — write/edit requirements: EARS, IDs, where content goes, prose, `Why:`
- **spec-change** — proposal → deltas → tasks → apply → archive
- **spec-review** — gate before committing docs
