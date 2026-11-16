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
| unit + surface | `grep -q '^fail-fast = false' .config/nextest.toml && cargo nextest run -P ci` | run |
| behaviour | `cargo test -p tau-test --test features` | run |
| steps current | `cargo run -p tau-test --bin steps -- --check` | run |
| dependencies current | `cargo deny check && grep -q '^[[:space:]]*- package-ecosystem: cargo$' .github/dependabot.yml && grep -q '^[[:space:]]*- package-ecosystem: github-actions$' .github/dependabot.yml` | run |
| workspace layout | `cargo metadata --no-deps` members match the crate list in `docs/stack.md`, each crate named after its directory, and the root manifest sets `resolver = "3"` | manual |
| no local state tracked | `f=/tmp/tau-tracked.$$ && git ls-files target .tau > $f && test ! -s $f; s=$?; rm -f $f; exit $s` | run |
| toolchain pinned | `v=$(grep -o 'channel = ".*"' rust-toolchain.toml \| cut -d'"' -f2) && test -n "$v" && test "$v" = "$(awk -F'\|' '{if (index($0,"Toolchain")==3) {split($4,a," "); print a[1]}}' docs/stack.md)"` the pinned channel is the version the `Toolchain` row of `docs/stack.md` states first, that row and no other, compared as whole text | run |
| core has no UI logic | `f=/tmp/tau-tree.$$ && cargo tree -p tau-core --all-features > $f && ! grep -qe gpui -e tau-app -e tau-acp $f; s=$?; rm -f $f; exit $s` | run |
| core has no runtime | `f=/tmp/tau-tree.$$ && cargo tree -p tau-core --all-features > $f && ! grep -qF -e tokio -e smol -e async-std -e async-io -e mio -e polling -e io-uring -e tokio-uring -e epoll -e glommio -e monoio -e embassy-executor -e concurrent-local-executor -e futures-executor -e async-executor -e async-task -e pollster -e rayon $f; s=$?; rm -f $f; exit $s` | run |
| core spawns nothing | `test -d crates/tau-core && ! grep -rqF -e '::spawn(' -e '.spawn(' -e 'spawn_obj' -e 'spawn_local' -e 'spawn_blocking' -e 'LocalPool' -e 'LocalSet' -e 'LocalExecutor' -e 'LocalSpawner' -e 'TaskPool' -e 'thread::Builder' -e 'thread::scope' -e 'process::Command' -e 'block_on' -e 'run_until' -e 'runtime::Builder' -e 'Runtime::new' crates/tau-core` | run |
| core has no HTTP client | `f=/tmp/tau-tree.$$ && cargo tree -p tau-core --all-features > $f && ! grep -qF -e reqwest -e hyper -e ureq -e rustls -e surf -e awc $f; s=$?; rm -f $f; exit $s` | run |
| core installs no subscriber | `f=/tmp/tau-tree.$$ && cargo tree -p tau-core --all-features > $f && ! grep -qF -e tracing-subscriber -e tracing-appender -e env_logger -e log4rs $f; s=$?; rm -f $f; exit $s` | run |
| live output under load | `cargo run -p tau-app --example tape_render -- --tape <f> --storage` renders at full token rate with a writer and a checkpoint running, inside the frame budget `spike_gpui_surface` sets, and the render path reads no storage | M0 |
| lints | `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings` | run |
| headless session | `cargo run -p tau-test --example headless -- --tape <f>`, a session run with no surface | M0 |
| desktop | `cargo run -p tau-app` (stub bin, no surface) | M0 |
| eval (never CI) | `cargo run -p tau-test --bin eval -- --suite crates/tau-test/suites/<x>.md` (no `eval` bin, no `suites/`) | M0 |

Authoring and review rules belong to the skills; don't restate them here.

## Working tree

Land on `main`: signed commits, one committable unit each, linear history (rebase, never a merge commit). Commit body names the change id and the gate status, including `skipped (M0)`.
A branch is only for work that can't land today (a spike with a build gate, a migration). Name it after the change id; delete it at land.
No worktrees by default. Trigger: two writer agents running at once, or a bisect/repro that needs a clean tree. Then one worktree per writer at `../tau-<id>` (never inside the repo), with `CARGO_TARGET_DIR=$HOME/.cache/tau-target` shared, so disk stays 1x and builds serialize. Create and remove it in the same session; leave no uncommitted state.

## Skills

`.agents/skills/` holds spec-authoring, spec-change, and spec-review; the harness loads each by description, so read a file only when its description matches.
