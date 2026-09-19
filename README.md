# Tau

A local-first coding agent: an agent loop with streaming chat, tree sessions with in-place branching, OM-based compaction, native sub-agents and tasks, and a desktop GUI (Tauri + Svelte). Built to the approved v0 spec: [`docs/spec/v0.md`](docs/spec/v0.md), with the decisions in [`docs/adr/`](docs/adr/) and the vocabulary in [`CONTEXT.md`](CONTEXT.md).

## Quickstart (~10 minutes, macOS)

Prerequisites: Rust (the pinned toolchain is in `rust-toolchain.toml`), Node 22+, and the Xcode command-line tools (macOS).

```sh
git clone git@github.com:aaronlockhartdev/tau.git
cd tau
./build
```

`./build` does the release cargo build, the Svelte build (with `svelte-check`), and the Tauri bundle. It prints where the artifact lands:

```
target/release/bundle/macos/Tau.app
```

Point the app at a model, then run it:

```sh
mkdir -p ~/.config/tau
cp dev/config.toml ~/.config/tau/config.toml
open target/release/bundle/macos/Tau.app
```

`dev/config.toml` targets the project's test endpoint (`https://llms.aaronlockhart.dev/v1`, model `qwen3.8-27b`). To use your own provider, edit that file (or drop a `.tau/config.toml` into a project — the project layer wins per provider name): each `[providers.<name>]` entry is a `base_url` + `key_env` (the name of an environment variable holding the key) + `models`; `[om].om_model` names the compaction model. Tau speaks the OpenAI-compatible `responses` endpoint only.

On Linux, `./build` builds the core and the frontend (the app bundle needs the webkit system libraries — that's v1 packaging work).

## Acceptance

```sh
./build
TAU_LIVE=1 ./scripts/acceptance.sh
```

The script proves the spec §1 in-scope list and prints PASS/FAIL/SKIP per leg: the built app launches (macOS), a live multi-turn session uses all four core tools with a verified golden-file edit, a model-spawned sub-agent works its task and wakes the parent, OM compaction runs live on a long session, branching + manual archive round-trips offline, and the 10k-entry performance demo (two live 25 ms streams, 25 ms coalescing) passes. Live legs are env-gated (`TAU_LIVE=1`, defaults to the dev endpoint; every live generation capped at 300 output tokens) and print SKIP when the endpoint is unavailable — a skip is not a failure.

The performance bar is the GUI's `?demo=1` mode (`app/verify-demo.mjs` is its committed, re-runnable verification):

```sh
node app/scripts/verify-demo.mjs
```

and a human click-through: open the built app, append `?demo=1` to the window URL (dev mode: `npm run dev` in `app/`), and scroll the 10k-entry session while the two streams run — the status bar shows the render range, render time, and stream count.

## Repository layout

- `crates/tau-core` — the standalone core library (no GUI dependencies, ADR-0002): the agent loop, the four hash-anchored tools, session storage (JSONL + per-line CRC + zstd sidecars), OM compaction, sub-agents, tasks, the provider client.
- `crates/tau-protocol` — the transport-agnostic core↔GUI message set (spec §8).
- `crates/tau-acceptance` — the acceptance driver (this README's live legs).
- `app/` — the Tauri + Svelte 5 GUI; `svelte/` is the frontend, `src-tauri/` the thin dispatch binding.
- `prototype/gui-ia/index.html` — the GUI's behavioral reference (the design baseline, kept as a spec artifact).
- `dev/config.toml` — the example/dev config.
- `scripts/` — acceptance; `.github/workflows/ci.yml` — CI (macOS: full build; Linux: the non-GUI surface).
