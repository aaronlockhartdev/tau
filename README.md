# Tau

A local-first coding agent: an agent loop with streaming chat, tree sessions with in-place branching, OM-based compaction, native sub-agents and tasks, and a desktop GUI (Tauri + Svelte). Built to the approved v0 spec: [`docs/spec/v0.md`](docs/spec/v0.md), with the decisions in [`docs/adr/`](docs/adr/) and the vocabulary in [`CONTEXT.md`](CONTEXT.md).

## Quickstart (~10 minutes, macOS)

Prerequisites: Rust (the pinned toolchain is in `rust-toolchain.toml`), Node 22+, `just` (the build entry point), and the Xcode command-line tools (macOS).

```sh
git clone git@github.com:aaronlockhartdev/tau.git
cd tau
just build
```

`just build` does the release cargo build, the Svelte build (with `svelte-check`), and the Tauri bundle. It prints where the artifact lands:

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

On Linux, `just build` builds the full app and bundles an AppImage (needs the webkit system libraries: `libwebkit2gtk-4.1-dev` + `libgtk-3-dev`); the launch smoke (leg a) runs under `xvfb`.

## Acceptance

```sh
just build
TAU_LIVE=1 just acceptance
```

`just acceptance` proves the spec §1 in-scope list and prints PASS/FAIL/SKIP per leg (a leg filter argument runs a subset): the built app launches (macOS and Linux), a live multi-turn session uses all four core tools with a verified golden-file edit, a model-spawned sub-agent works its task and wakes the parent, OM compaction runs live on a long session, branching + manual archive round-trips offline, and the 10k-entry demo entry (two live 25 ms streams, 25 ms coalescing) passes. Live legs are env-gated (`TAU_LIVE=1`, defaults to the dev endpoint; every live generation capped at 300 output tokens) and print SKIP when the endpoint is unavailable — a skip is not a failure.

The performance bar is a dev-only demo entry — the 10k-entry fixture + two 25 ms streams behind `app/demo.html`, excluded from the release build (`app/scripts/verify-demo.mjs` is its committed, re-runnable verification):

```sh
node app/scripts/verify-demo.mjs
```

and a human click-through: `npm run dev` in `app/`, open `http://localhost:5173/demo.html`, and scroll the 10k-entry session while the two streams run — the status bar shows the render range, render time, and stream count.

## Repository layout

- `crates/tau-core` — the standalone core library (no GUI dependencies, ADR-0002): the agent loop, the four hash-anchored tools, session storage (JSONL + per-line CRC + zstd sidecars), OM compaction, sub-agents, tasks, the provider client.
- `crates/tau-protocol` — the transport-agnostic core↔GUI message set (spec §8).
- `crates/tau-acceptance` — the acceptance driver (this README's live legs).
- `app/` — the Tauri + Svelte 5 GUI; `svelte/` is the frontend, `src-tauri/` the thin dispatch binding.
- `prototype/gui-ia/index.html` — the GUI's behavioral reference (the design baseline, kept as a spec artifact).
- `dev/config.toml` — the example/dev config.
- `justfile` — the build/acceptance entry point (`just build`, `just test`, `just acceptance`); `.github/workflows/ci.yml` — CI (rust matrix, frontend, macOS app + launch smoke, Linux acceptance).
