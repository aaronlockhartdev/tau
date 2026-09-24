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

On Linux, `just build` builds the full app and bundles an AppImage (needs the webkit system libraries: `libwebkit2gtk-4.1-dev` + `libgtk-3-dev`); the launch smoke (the `launch` suite) runs under `xvfb`.

For the development loop — and for tauri-pilot debugging, whose socket is wired into debug builds only — `just dev` runs a debug build with the Svelte dev server and hot reload.

## Acceptance

```sh
just build
TAU_LIVE=1 just acceptance
```

`just acceptance` proves the spec §1 in-scope list and prints PASS/FAIL/SKIP per suite (a suite filter argument runs a subset): the built app launches (macOS and Linux), a live multi-turn session uses all four core tools with a verified golden-file edit, a model-spawned sub-agent works its task and wakes the parent, OM compaction runs live on a long session, branching + manual archive round-trips offline, and the real app on the 10k-entry shared fixture (two deterministic 25 ms streams, 25 ms coalescing) passes. The live suites are env-gated (`TAU_LIVE=1`, defaults to the dev endpoint; every live generation capped at 300 output tokens) and print SKIP when the endpoint is unavailable — a skip is not a failure.

The performance bar is the `e2e` suite — the debug app on the shared 10k-entry fixture (the same `target/test-fixture/session.jsonl` the Rust tests use), driven with WebdriverIO — the official Tauri E2E stack, `@wdio/tauri-service` + the embedded `tauri-plugin-wdio-webdriver` provider (roadmap G2; tauri-pilot stays the agent's direct-interaction route, AGENTS.md) — two deterministic 25 ms streams coalesced at 25 ms (spec §8):

```sh
just acceptance e2e
```

and a human click-through: `npm run dev` in `app/`, open a workspace with a large session, and scroll while streams run — the status bar shows the render range, render time, and stream count.

## Repository layout

- `crates/tau-core` — the standalone core library (no GUI dependencies, ADR-0002): the agent loop, the four hash-anchored tools, session storage (JSONL + per-line CRC + zstd sidecars), OM compaction, sub-agents, tasks, the provider client.
- `crates/tau-protocol` — the transport-agnostic core↔GUI message set (spec §8).
- `crates/tau-acceptance` — the acceptance driver (this README's live suites).
- `app/` — the Tauri + Svelte 5 GUI; `svelte/` is the frontend, `src-tauri/` the thin dispatch binding.
- `prototype/gui-ia/index.html` — the GUI's behavioral reference (the design baseline, kept as a spec artifact).
- `dev/config.toml` — the example/dev config.
- `justfile` — the build/acceptance entry point (`just build`, `just test`, `just acceptance`); `.github/workflows/ci.yml` — CI (rust matrix, frontend, macOS app + launch smoke, Linux acceptance).
