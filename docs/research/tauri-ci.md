# Research: Tauri testing and CI best practices

Researched 2026-09-24, in response to eight consecutive red E2E CI runs where each failure moved one line later (a starved CI webview outlasting the tauri-pilot plugin's fixed 10 s `eval` budget).

**Question.** What do the Tauri ecosystem and its practitioners consider good practice for testing a Tauri app in CI — and does it change what tau runs in CI vs locally?

**Short answer.** The official stack is Rust unit tests on the mock runtime + **WebDriver-protocol E2E** (WebdriverIO + `@wdio/tauri-service`, with an *embedded* in-app WebDriver server as the default provider — the only route that supports macOS). Every practitioner who runs full-app E2E in CI runs it headless (xvfb + `webkit2gtk-driver` on Linux, no driver on macOS via the embedded provider) and treats **flakiness as an inherent property of software-rendered WebKit in uncontrolled runners** — mitigated by artifacts, retries, and scoping, never eliminated. Nobody claims deterministic full-app E2E on CI. That settles the split: CI owns *lean* real-app coverage that can't plausibly outlast a single interaction budget; local owns the stress/performance E2E.

---

## 1. The official stack (tauri.app docs, v2, current)

[Tests overview](https://v2.tauri.app/develop/tests/): unit/integration testing uses a **mock runtime** ("native webview libraries are not executed"); E2E is explicitly the **WebDriver protocol**.

[WebDriver guide](https://v2.tauri.app/develop/tests/webdriver/):

- The **recommended** path is **WebdriverIO + `@wdio/tauri-service`**, working on Windows, Linux, and macOS.
- Default provider: an **embedded WebDriver server inside the app** (`tauri-plugin-wdio-webdriver`) — no external driver binary, and **this is how macOS is supported** (macOS has no WKWebView driver tool for the external route).
- Alternative: drive the platform's native driver — `tauri-driver` (WebKitGTK) on Windows/Linux; [CrabNebula](https://crabnebula.dev) (a cross-platform `tauri-driver` fork, paid API key for macOS) anywhere.
- The service adds: `browser.tauri.execute()` (JS in the app), **IPC command mocking**, frontend+backend **log capture**, multiremote.
- **Browser mode**: the frontend runs in plain Chrome against a Vite dev server with `invoke()` intercepted — "for fast, renderer-only tests … no Tauri binary, driver, or plugin required."

[WebDriver CI guide](https://v2.tauri.app/develop/tests/webdriver/ci/): the canonical GitHub Actions job runs `cargo test` **before** the WebDriver tests ("to avoid testing a broken application"), and on Linux installs **`webkit2gtk-driver` and `xvfb`** in addition to the build libs — the WebKitWebDriver binary ships in a *separate* package from `libwebkit2gtk-4.1-dev`.

## 2. Community practice

- [short-circuit/stratum PR #183](https://github.com/short-circuit/stratum/pull/183): a dedicated **`e2e-harness` CI job** — build app, install `tauri-driver`, run the E2E suite, upload `test-results/*` + the built binary **as artifacts on failure**. Installs `webkit2gtk-driver` + `xvfb` separately from the build deps.
- [Lukydemboy/megadesk PR #5](https://github.com/Lukydemboy/megadesk/pull/5): "CI-only E2E testing via WebdriverIO + tauri-driver," Xvfb headless; **screenshot after each significant render + automatic screenshot on failure**, uploaded from the workflow ("no live visual feed to watch").
- [anchapin/planar-nexus #1895](https://github.com/anchapin/planar-nexus/issues/1895): a maintained record of **residual webkit e2e failures on nightly CI** — WebKit timeouts, worker-init ordering, stricter same-origin behavior "in the Tauri WebView context," fixed with `page.waitForFunction`-style polls. Evidence that the standard stack has the same flaky tail tau is hitting.
- [mpiton](https://dev.to/mpiton/i-built-a-cli-to-test-tauri-apps-because-nothing-else-worked-3915) (tauri-pilot's author): built tauri-pilot *because* the WebdriverIO route cost him two hours to connect — Node deps, `wdio.conf.ts`, **matching the WebDriver binary to the WebKit version**, flaky selectors — and because Playwright can't drive WebKitGTK. tauri-pilot is the documented alternative, positioned for agent-driven shell workflows.

## 3. What this means for tau

1. **tauri-pilot is a legitimate *direct-interaction* route, not a hack.** It is the author's purpose-built alternative to the WDIO stack, positioned for agent-driven shell workflows (AGENTS.md "The line": the agent's route to the running app). The flakiness tau hit on the pilot route was **not** a tauri-pilot defect: planar-nexus documents the same webkit-on-CI failures with the *standard* stack. The starved, software-rendered webview is the environment — and the standard stack's framework-level waiting (point 2) is what addresses the budget wall structurally.
2. **The one structural advantage the standard stack has is WebDriver's standardized waiting semantics** — auto-wait, explicit condition waits, framework-level command timeouts — versus tau's raw JSON-RPC `eval`, whose plugin-side budget is a fixed 10 s with no per-call override. That is the exact wall tau's eight red runs hit. (Adopted as the test route — roadmap G2.)
3. **Consensus on what CI is *for*:** boot-on-platform proof, bridge health, and a *lean* functional pass — with artifacts on failure, and stress/performance left to controlled (local) environments. tau's current CI E2E (10k fixture, two streams, performance bar) is a stress test; that is the mismatch, not the tooling.

## 4. Decision (2026-09-24)

**Division of labor (user, 2026-09-24):**

- **Automated testing (the `e2e` suite, CI) uses the official stack** — WebdriverIO + `@wdio/tauri-service`, with the **embedded** `tauri-plugin-wdio-webdriver` provider (debug builds only), configured exactly per the [WebDriver guide](https://v2.tauri.app/develop/tests/webdriver/) and the [CI guide](https://v2.tauri.app/develop/tests/webdriver/ci/): the debug binary under test, `cargo test` before E2E, xvfb on Linux, **framework-level timeouts and auto-wait** (standardized waiting semantics — the structural wall tau's eight red runs hit, the pilot plugin's fixed 10 s `eval` budget, disappears with it), artifacts on failure. Roadmap item **G2** is the migration; `run-e2e.mjs` retires — no second implementation kept "just in case".
- **tauri-pilot remains the agent's route** to the running app — direct interaction: debugging, dogfooding, screenshots — formalized in AGENTS.md ("The line"). It is no longer an E2E driver.

The lean/stress split stands: **CI** runs a lean **1,000-entry fixture** mode (one stream, no performance checks); **local** `just acceptance e2e` runs the full 10k / two-stream / performance-bar suite where the §8 bar is meaningful. Residual WebKit flakiness in CI stays an environment property, mitigated by artifacts and scoping — never claimed away by the driver.

Sources: tauri.app v2 docs (Tests / WebDriver / WebDriver-CI), the tauri-docs `ci.md` workflow, stratum #183, megadesk #5, planar-nexus #1895, mpiton's tauri-pilot rationale.
