# Research: Tauri 2 best practices for the refactor

Researched 2026-10-01. All claims are cited to primary sources fetched during this task: the official v2 docs (v2.tauri.app), the tauri-apps source/issues, the docs.rs API reference, and first-party plugin docs. Repo grounding: `app/src-tauri` (the full Tauri surface is `src/lib.rs` + `src/main.rs`), `capabilities/default.json`, `tauri.conf.json`, `build.rs`, ADR-0002/0006.

**Question.** What should the refactor enforce on the Tauri side — IPC/command design, security-model usage, state ownership between core and glue, window management, config, and error surfaces across the IPC boundary — and is there Tauri-specific code-quality tooling beyond the already-adopted set (clippy/deny/Miri/fuzz, the ACL system)?

**Short answer.** The codebase's shape is already close to what the official docs prescribe: one async command carrying a typed protocol, state owned by the core, a 3-permission capability, feature-gated e2e plugins. The refactor should **codify that shape as rules** so it can't erode, and close **one real gap: `csp: null`** (CSP protection is off — the docs say it is only enabled when set, and should be as restrictive as possible). No new Tauri-specific tooling is warranted: the ACL system is the Tauri-specific security tool and it is adopted; capability files are build-time validated; unit tests already run on the official mock runtime.

---

## 1. Candidate AGENTS.md rules (verdicts)

### R1 — Set a restrictive CSP; `csp: null` is a violation. **ADOPT** (the one real gap)

[security/csp](https://v2.tauri.app/security/csp/): "The CSP protection is **only enabled if set** on the Tauri configuration file. You should make it as restricted as possible, only allowing the webview to load assets from hosts you trust, and preferably own." Local scripts are hashed and nonces are appended at compile time; remote/CDN content "introduce an attack vector".

`tauri.conf.json` currently sets `"csp": null` → no CSP protection at all. Rule: `app.security.csp` must be a restrictive policy (the docs' [examples/api](https://github.com/tauri-apps/tauri/blob/dev/examples/api/src-tauri/tauri.conf.json) shape: `default-src 'self' customprotocol: asset:`, `connect-src ipc: http://ipc.localhost`, plus only what the app needs — e.g. `blob:`/`data:` in `img-src` for file previews).

### R2 — Global state and business logic live in the core process; the frontend stays a thin renderer. **ADOPT** (codifies ADR-0002)

[concept/process-model](https://v2.tauri.app/concept/process-model/): "The Core process should also be responsible for managing global state, such as settings or database connections. This allows you to easily synchronize state between windows and protect your business-sensitive data from prying eyes in the Frontend." And: "you must always sanitize user input, never handle secrets in the Frontend, and ideally defer as much business logic as possible to the Core process to keep your attack surface small."

Already the design (ADR-0002: standalone core crate, thin glue). The rule makes it enforceable: new glue code must not introduce window-scoped business state or logic on the frontend side of the seam.

### R3 — Commands are `async`; never hold a `State` guard across `.await`/blocking work. **ADOPT** (seam already compliant)

[develop/calling-rust](https://v2.tauri.app/develop/calling-rust/): "Async commands are preferred in Tauri to perform heavy work in a manner that doesn't result in UI freezes or slowdowns. Async commands are executed on a separate async task … Commands without the *async* keyword are executed on the main thread." Borrowed arguments — explicitly including `State<'_, Data>` — are unsupported in async command signatures ([upstream issue #2533](https://github.com/tauri-apps/tauri/issues/2533)); the documented workaround is converting to owned types.

`tau_command` already does exactly this: `async`, takes `State<'_, CoreState>`, clones the `Arc<Core>` out, then `spawn_blocking` (lib.rs — with a comment explaining why a wedged dispatch must not tie up a runtime worker). Rule: new/changed commands stay `async` and extract owned handles before any blocking/await work.

### R4 — Errors cross IPC as serializable, tagged protocol errors. **ADOPT** (satisfied; rule prevents erosion)

[develop/calling-rust, Error Handling](https://v2.tauri.app/develop/calling-rust/): everything a command returns — including errors — must implement `serde::Serialize`; the docs recommend a custom error type (thiserror + manual `Serialize`) with `#[serde(tag = "kind", content = "message")]` so the frontend can "map it to a similar looking TypeScript error enum", because a typed error set "mak[es] all possible errors explicit".

Already satisfied: the seam returns `Result<CommandOutput, ProtocolError>` (ADR-0006's typed protocol). Rule: no ad-hoc `String` error paths at the seam; `ProtocolError::Other` stays the last-resort escape hatch only.

### R5 — High-frequency core→UI streams are coalesced before `emit`, or delivered via a `Channel`. **ADOPT** (pump already compliant)

[develop/calling-frontend](https://v2.tauri.app/develop/calling-frontend/): the event system "is designed for situations where small amounts of data need to be streamed" and "is **not designed for low latency or high throughput** situations"; under the hood it "directly evaluates JavaScript code so it might not be suitable to sending a large amount of data". Channels "are designed to be fast and deliver ordered data … for streaming operations such as download progress, child process output and WebSocket messages". [concept/inter-process-communication](https://v2.tauri.app/concept/inter-process-communication/): events are "fire-and-forget, one-way … best suited to communicate lifecycle events and state changes".

The `tau://event` pump already emits coalesced batches (spec §8's 25 ms coalescing keeps frequency bounded). Rule: a raw per-item `emit` loop for a high-frequency stream is a violation — batch it, or use `tauri::ipc::Channel`.

### R6 — Capabilities: individual files, narrowest individual permissions, no `:default` sets without justification. **ADOPT** (already compliant)

[security/capabilities](https://v2.tauri.app/security/capabilities/): "It is good practice to use individual files and only reference them by identifier in the `tauri.conf.json`"; windows in multiple capabilities "effectively merge the security boundaries". [plugin/dialog](https://v2.tauri.app/plugin/dialog/): `dialog:default` = `allow-message` + `allow-save` + `allow-open`, while individual `allow-*` permissions exist for narrower grants.

`capabilities/default.json` already grants exactly `core:event:allow-listen`, `core:event:allow-unlisten`, `dialog:allow-open` — the narrow shape. Rule: new grants name individual `allow-*` permissions; a `:default` set needs a reason in the file's `description`.

### R7 — Registered commands are allowed by default: the command set *is* the IPC attack surface. **ADOPT**

[security/capabilities](https://v2.tauri.app/security/capabilities/): "By default, all commands that you registered in your app (using `tauri::Builder::invoke_handler`) are allowed to be used by all the windows and webviews of the app. To change that, consider using `AppManifest::commands`". [security/](https://v2.tauri.app/security/): webview code reaches the OS "via the well-defined IPC layer" configured by capabilities.

The single-command protocol (ADR-0006) is the control: one surface, fully typed. Rule: adding a `#[tauri::command]` is a deliberate, reviewable decision (it is exposed to the webview with no permission entry); if the seam is ever widened, the commands are enumerated via `AppManifest::commands` and scoped per capability.

### R8 — Window creation happens from async context. **ADOPT** (dormant — v0 is single-window)

[docs.rs `WebviewWindowBuilder`](https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindowBuilder.html): "On Windows, this function **deadlocks** when used in a synchronous command and event handlers … You should use async commands and separate threads when creating windows."

No creation path exists in v0 (single window in `tauri.conf.json`). Rule kept short so the footgun is documented before multi-window work lands.

### R9 — `withGlobalTauri: true` stays; document why. **SKIP** (no change)

[reference/config](https://v2.tauri.app/reference/config/): `withGlobalTauri` — "Whether we should inject the Tauri API on `window.__TAURI__`". The app source consumes only `@tauri-apps/api` imports (`app/svelte/lib/protocol.ts`); the global is required by the **tauri-pilot** bridge (the agent's dogfood route, debug/e2e builds only — its eval path throws "Make sure withGlobalTauri is enabled" without it; [mpiton/tauri-pilot](https://github.com/mpiton/tauri-pilot)). `tauri.conf.json` is static — the flag cannot be feature-gated — so it must stay `true` while pilot is in the stack. Verdict: keep, with the reason recorded here; the webview-visible global is accepted v0 surface.

### R10 — Everything crossing the IPC boundary arrives as a typed protocol value. **ADOPT** (satisfied by ADR-0006)

[security/](https://v2.tauri.app/security/): "Inspecting and strongly defining all data passed between boundaries is very important to prevent trust boundary violations." [concept/inter-process-communication](https://v2.tauri.app/concept/inter-process-communication/): commands use a "JSON-RPC like protocol … all arguments and return data must be serializable to JSON".

The typed `Command`/`CommandOutput` union (ADR-0006) *is* the inspection point. Rule: no raw `serde_json::Value` or untyped argument at the seam.

### Verdict summary

| # | Rule | Verdict |
|---|---|---|
| R1 | Restrictive CSP; `csp: null` forbidden | **ADOPT** — real gap, fix in the refactor |
| R2 | State + business logic in the core process | ADOPT — codifies ADR-0002 |
| R3 | Async commands; owned handles before blocking work | ADOPT — seam compliant |
| R4 | Tagged serializable errors across IPC | ADOPT — satisfied by `ProtocolError` |
| R5 | Coalesced events or Channels for streams | ADOPT — pump compliant |
| R6 | Individual permission grants, no `:default` sets | ADOPT — compliant |
| R7 | Command set = attack surface; deliberate additions | ADOPT — single seam is the control |
| R8 | Window creation from async context | ADOPT — dormant (single-window v0) |
| R9 | `withGlobalTauri: true` | SKIP — keep, document the pilot dependency |
| R10 | Typed protocol values at the boundary | ADOPT — satisfied by ADR-0006 |

## 2. Tooling candidates (beyond the adopted set)

Already adopted, for reference: clippy `-D warnings`, cargo-deny, Miri (tau-protocol), cargo-fuzz, insta, proptest, cargo-llvm-cov, the `file-size` CI job, ESLint + svelte-check, and the ACL/capability system itself.

- **Standalone ACL/capability audit CLI** — **SKIP**. None exists among the fetched first-party sources. The mechanisms that do exist are already in play: `tauri-build` resolves permissions at build time and hard-fails on unresolvable ones (the basis of this repo's feature-gated `build.rs` capability generation), and capability files carry a `$schema` reference to the generated `gen/schemas/desktop-schema.json` for editor validation.
- **Mock runtime for unit tests** — **already adopted**. `app/src-tauri/tests/tauri_smoke.rs` runs the one real seam on `tauri::test::mock_app` (the official unit-test route per the [tests overview](https://v2.tauri.app/develop/tests/)); the rest of the surface is tested transport-free through `tau_core::harness`.
- **Mobile/platform-specific linters** — out of scope: the v0 spec has no mobile surface (bundle targets are `app`/`appimage`/`dmg`), so mobile best-practice tooling does not apply.

**Net: no new tooling. The refactor's Tauri work is one config fix (R1) plus codifying the existing shape as rules (R2–R8, R10).**
