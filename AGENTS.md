# tau: agent guide

Agent-facing. `README.md` is user-facing; nothing here belongs there.
Auto-loaded: this file (nested `AGENTS.md` files, nearest wins) and each skill's name + description. Everything else is read by path, on demand.

## Read order for a task

1. This file.
2. `spec/index.md`: cross-cutting invariants and the layout table, then the 1–2 domains the task touches.
3. That domain's `spec.md`, then only the leaf `spec.md` you need.
4. Changing behaviour? `changes/<id>/` if one exists; if not, open one (skill: spec-change).
5. A skill, when the task matches its description.

Read nothing else by default. If the load feels heavy, you are reading files you don't need.
Every fact a task needs lives elsewhere by kind: rules in `spec/`, terms in `docs/glossary.md`, pins in `docs/stack.md`, steps in `crates/tau-test/tests/features/STEPS.md`. This file carries commands and the rules that bind every task.

## Gates

`run` = works today. `M0` = created by the M0 scaffold; report `skipped (M0)`, never fake a pass. `manual` = checked by an agent, never in CI. A change merges when every `run` gate is green and the rest are reported skipped.

| Gate | Command | State |
|---|---|---|
| ci lane | `cargo run -p tau-tools --bin gates` runs every other `run` row in this table | run |
| spec tool | `cargo run -p tau-tools --bin spec -- count` smoke | run |
| spec lint | `cargo run -p tau-tools --bin spec -- --check` | run |
| contracts current | `cargo run -p tau-protocol --bin contracts -- --check` | run |
| doc lint | `rumdl check spec changes docs AGENTS.md README.md` | run |
| links | `lychee spec changes docs AGENTS.md README.md` | run |
| unit + surface | `cargo nextest run -P ci` | run |
| behaviour | `cargo test -p tau-test --test features` | run |
| steps current | `cargo run -p tau-test --bin steps -- --check` | run |
| dependencies current | `cargo deny check && test -f .github/dependabot.yml` | run |
| workspace layout | `cargo metadata --no-deps` members match the crate list in `docs/stack.md`, each crate named after its directory, and the root manifest sets `resolver = "3"` | manual |
| no local state tracked | `git ls-files target .tau > /tmp/tracked && test ! -s /tmp/tracked` | run |
| toolchain pinned | `v=$(grep -o 'channel = ".*"' rust-toolchain.toml \| cut -d'"' -f2) && test -n "$v" && grep -qF "$v" docs/stack.md` the pinned channel is the stable release `docs/stack.md` names | run |
| core has no UI logic | `cargo tree -p tau-core > /tmp/core-tree && ! grep -qe gpui -e tau-app -e tau-acp /tmp/core-tree` | run |
| live output under load | `cargo run -p tau-app --example tape_render -- --tape <f> --storage` renders at full token rate with a writer and a checkpoint running, inside the frame budget `spike_gpui_surface` sets, and the render path reads no storage | M0 |
| lints | `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings` | run |
| headless session | `cargo run -p tau-core --example run -- --tape <f>` | M0 |
| desktop | `cargo run -p tau-app` (stub bin, no surface) | M0 |
| eval (never CI) | `cargo run -p tau-test --bin eval -- --suite crates/tau-test/suites/<x>.md` | manual |

Authoring and review rules belong to the skills; don't restate them here.

## Working tree

Land on `main`: signed commits, one committable unit each, linear history (rebase, never a merge commit). Commit body names the change id and the gate status, including `skipped (M0)`.
A branch is only for work that can't land today (a spike with a build gate, a migration). Name it after the change id; delete it at land.
No worktrees by default. Trigger: two writer agents running at once, or a bisect/repro that needs a clean tree. Then one worktree per writer at `../tau-<id>` (never inside the repo), with `CARGO_TARGET_DIR=$HOME/.cache/tau-target` shared, so disk stays 1x and builds serialize. Create and remove it in the same session; leave no uncommitted state.

## Skills

`.agents/skills/` holds spec-authoring, spec-change, and spec-review; the harness loads each by description, so read a file only when its description matches.
