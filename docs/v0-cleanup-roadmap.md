# Tau v0 — codebase clean-up roadmap

Prepared ahead of v0. Three parallel explorers (testing, agent-harness, code) produced the findings this
roadmap synthesizes. This document is a **plan of decision tickets and work items**, not a mandate to do
everything at once; it names what to do, in what order, and what to deliberately *not* do.

Full research reports (kept outside the repo):
- `/tmp/tau-cleanup/01-testing.md` — testing story
- `/tmp/tau-cleanup/02-agent-harness.md` — ADR / agent-doc / prototype audit
- `/tmp/tau-cleanup/03-code.md` — architecture, comment audit, dead code

Working vocabulary comes from `CONTEXT.md` (domain) and the codebase-design skill (module, interface,
depth, seam, adapter, leverage, locality).

---

## How to work this roadmap

Each phase is a set of small, independently-landable tickets. A ticket is one branch, one red→green,
landed on its own. Nothing here is speculative scaffolding between steps (per `AGENTS.md`).

No open **decisions** remain in this roadmap: the C1 harness move is **decided** (user, 2026-09-23) —
the seam is that `tau-core` owns all logic necessary for a non-Tauri interface and the Tauri app owns all
Tauri-specific code, so whatever sits on the wrong side moves — and the orphaned tickets #28–#33 are
**left as standalone tickets** (user, 2026-09-23). Everything here is mechanical work.

Order: **Phase 0 (quick wins) → Phase 1 (testing) → Phase 2 (architecture)**. Phase 0 now also carries the
build/acceptance tooling change (F), the demo-rig→test-suite formalization (G), and the file-size policy (H);
Phase 2's C1 (the harness move) is the strategic piece and must land before v0 freeze.

---

## Phase 0 — quick wins (each a small ticket, ~1–3 h apiece)

### A. Comment audit — delete "what", keep "why"

The Rust source is disciplined (nearly every comment carries a spec/ADR/review citation). The offenders
concentrate in section-divider banners, one stale doc line, and a handful of Svelte "what" comments.
**53 flagged comments: 42 banners/section-dividers, 9 what/restating/narration, 1 stale duplicate, 1
unexplained dead write.** Rules: `AGENTS.md` (a comment is justified only by a *why*; no narration, no name
restatement, no banners, no "Adds/Updates X", no TODO without ticket+reason).

Do (delete the banner, keep any why-text):
- `app/src-tauri/src/core.rs` L716, L1046, L1605 (`── workspaces ──`, `── sessions ──`, `── the dispatch surface ──`)
- `crates/tau-core/src/subagent.rs` L1390, L1658, L1855, L2072, L2328, L2717, L2903 (the `── … ──` dividers)
- `crates/tau-core/src/task.rs` L410, L654; `crates/tau-acceptance/src/main.rs` L104/153/370/493/551 (leg banners)
- `scripts/acceptance.sh` L60/97/100/103/106/114 (leg dividers already in the header doc)
- `app/scripts/verify-demo.mjs` L374/670/688/720/875/887/900/942/996/1045/1066 + the `--- …` check dividers at
  L447/465/493/522/531/535/560 (strip the dashes, keep the why-text where it carries a regression name)
- `app/svelte/lib/store.svelte` L406

Rewrite-as-why (not delete): `store.svelte` L212–213 → `// TPS = a call's output tokens over its own stream
duration (the status bar's metric)`.

Delete (narration / name restatement): `store.svelte` L549–551, L158–159, L74–76, L77, L95, L121;
`app/svelte/components/StatusBar.svelte` L2–5 (drop the "moved to debug logs" change-narration, keep the
layout description); `app/svelte/lib/fixture.ts` L209.

Delete the stale duplicate doc line `crates/tau-core/src/subagent.rs` L410 (contradicts the L411 rewrite).

### B. Dead code — 8 hard-dead items

| # | Path | Action |
|---|---|---|
| 1 | `crates/tau-core/src/session.rs:214` `for_no_project` (0 callers) | delete fn + its tests |
| 2 | `crates/tau-core/src/session.rs:239` `with_blob_threshold` (tests only) | see C5 — wire the config or delete with it |
| 3 | `crates/tau-core/src/session.rs:467` `append_raw` (tests only, no raw path in v0) | delete fn + its tests |
| 4 | `crates/tau-core/src/session.rs:453` `append_compaction` (1 test caller; OM commits via `store.append`) | delete fn + its test |
| 5 | `app/src-tauri/src/core.rs:2700` `core.self_weak.replace(…)` re-stamp in `run_turn` (no-op; set once in `build` L599) | delete the write |
| 6 | `app/package.json:21` `tauri-agent-tools` devDependency (imported nowhere; `dev_bridge.rs` is a vendored copy, `agent-eval.mjs` is its replacement) | remove from package.json + lock |
| 7 | `app/svelte/lib/markdown.ts:13,73` `hl`, `export { esc }` (used only inside the module) | un-export |
| 8 | `app/svelte/lib/fixture.ts:17,25,30` types `DemoSession`, `FixturePendingMsg`, `FixtureSessionState` (no importers) | un-export |

**Intentionally kept** (do not delete — documented reserved surface, spec §8): `SystemEventKind::ProviderChanged`
(+ its TS mirror `protocol.ts:307`), `Command::ProviderAdd/Set/Delete` (dispatch → `Unsupported` in v0),
`Command::SessionFork/SessionBranch` (see C7), and the vendored `dev_bridge.rs`.

### C. Protocol mirror fix (C6) — one line + a guard

`app/svelte/lib/protocol.ts` `Command` union is missing `provider_delete` (dropped by drive-by in `55e6284`,
never restored — the same drift class as the `agents` `CommandOutput` a reviewer caught in `e79d705`).
Rust has `ProviderAdd/Set/Delete` (lib.rs L300–312).

- Add `| { type: 'provider_delete'; name: string }` to the TS `Command` union.
- Add a **mirror-diff check**: derive the TS `Command` tag list from the Rust serde tags (a small script run
  in CI) so the ADR-0006 field-for-field mirror can't silently drift again. This is the real win.

### D. ADR + agent-doc homogenization

Standardize all 7 ADRs on one template (see `02-agent-harness.md` §1.3): `# {title}` (no number prefix), one
dated `**Status**: accepted (date) — {provenance}`, then `Decision / Rationale / Consequences / Considered /
Revisit triggers / Supplement` as bold labels, only when they earn it. Rules: ticket references always
`[name](https://github.com/aaronlockhartdev/tau/issues/N)` (a name wraps its link, never a bare `#N`);
"the map" always named; no wayfinder process words ("fog", "frontier", "charting") in ADR bodies.

Per-file edits (full list in `02-agent-harness.md` §1.4):
- **0001** — add dated Status; delete the stale "…is open on the map" sentence; name the map + settling
  tickets in the Supplement header.
- **0002** — add Status (accepted, settled at charting).
- **0003** — add Status; name map #1 + ticket #7 + research #4.
- **0004** — add Status; name research #2.
- **0005** — add Status; name research #3 + #13; "the ticket" → "that ticket".
- **0006** — add Status; name #13 + #10; "security model is fog" → "ADR-0007 settles the security model".
- **0007** — drop the `# 7.` prefix and standalone `Date:` line; `##` headings → bold labels; fold Date into
  Status; name the build-map #14 and disambiguate "spec §11 / U1".

**Companion spec fix:** `docs/spec/v0.md:255–259` reuses **U1 for two items** (context files and security) and
says "leaving three open" while listing four rows — renumber (e.g. U5) so ADR-0007's reference is unambiguous.

Agent-doc fixes (`02-agent-harness.md` §2):
- `AGENTS.md` — point the Domain-docs section at `docs/spec/v0.md` (the approved contract) as well as
  `CONTEXT.md`/`docs/adr/`; note maps/tickets live in GitHub Issues; drop the "Default" template residue.
- `docs/agents/domain.md` — delete the "File structure" template section (the `0001-event-sourced-orders.md`
  example is a foreign domain); fix the L35 example that mis-cites "ADR-0007 (event-sourced orders)" →
  ADR-0005 (session storage); drop the dead "proceed silently" branch.
- `docs/agents/issue-tracker.md` — tighten the Wayfinding-operations section: list the 5 map sections
  (add "Not yet specified" / "Out of scope"), give the exact `gh api` sub-issues + `blocked_by` recipes, add
  the **cross-repo gotcha** (`issue_id` is a global db id — verify a created edge stays in-repo; #10 carries
  a junk edge to sinatra/sinatra#1 today), make the frontier query executable, drop the dead PR-surface branch.
- `docs/agents/triage-labels.md` — drop "mattpocock/skills" vocabulary + the author-instruction line; **add a
  Wayfinder-labels section** (map + research/prototype/grilling/task); note 4 of 5 triage labels don't exist
  in the tracker yet (see E).

### E. Prototypes, temp docs, tracker hygiene

| Item | Action |
|---|---|
| `app/prototype/layout.html`, `palette.html`, `statusbar.html` | **Delete** — superseded throwaway prototypes (winners folded into the Svelte components in `376de61`/`c5d24b7`; zero references) |
| `prototype/gui-ia/index.html` | **Keep** — the design baseline, triple-documented (README, spec §9, code comments) |
| `app/demo.html` + `svelte/demo.ts` + `lib/fixture.ts` + `scripts/verify-demo.mjs` | **Keep** — the live verification rig (acceptance leg f, CI) |
| `app/scripts/agent-eval.mjs` | **Keep** — live dev tool; update the "tauri-agent-tools" reference in `docs/gui-bugs.md:3` |
| `docs/research/hash-anchors.md`, `remote-backend.md` | **Keep** — finished research, decision records |
| `docs/gui-bugs.md` | **Keep** as a historical log; verify + strike the 2 stale "Open" items |
| Branches `research/remote-backend`, `prototype/gui-ia-variants-a-c` | **Delete** (both are ancestors of main) |

Tracker actions (`gh`, read the current state first):
- Resolve **#33** (research committed in `793bafc`, ticket never closed): post the resolution comment + close.
- The **4 missing triage labels** (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`) don't
  exist in the tracker — `gh label create` them, or consciously drop those roles from the mapping.
- **Junk edge**: delete #10's cross-repo `blocked_by` edge to sinatra/sinatra#1.
- **Orphaned #28–#33** have no parent map — **left as-is** (user, 2026-09-23): standalone post-v0 tickets,
  not children of any map; no map will be charted for them.

### F. Build + acceptance tooling — retire `./build` + `./scripts/acceptance.sh`

Both are ad-hoc shell scripts whose entire job is to drive native workspace tools. Replace them with a
single entry point that delegates to the native tools (`cargo`, `npm`/`vite`, `tauri`). Runner: **just** (user
decision, 2026-09-23) — the modern command runner, cleaner syntax than make; CI installs it with the same
`taiki-e/install-action@v2` already used for nextest (`tool: just`). Targets:
- `build` — platform-aware: macOS → `cargo build --workspace --release` + `app: npm ci && npm run build` +
  `npx tauri build --bundles app`; Linux → the same with the Linux bundle target (item F2).
- `test` — `cargo nextest run --workspace` + the frontend suite (item G): what CI's rust + frontend jobs run.
- `acceptance [legs]` — the spec §1 in-scope legs: `cargo run --release --bin tau-acceptance -- <leg>` for
  b/c/d (live) and e (offline), the GUI E2E suite for f, and the launch smoke (a) on both platforms. Live
  legs stay `TAU_LIVE`-gated exactly as today; a skipped live leg is not a failure, a red leg is.
`./build` and `./scripts/acceptance.sh` are deleted; CI + README point at `just`.
### F2. Linux GUI support — spec §13 is "macOS first, Linux second", not "Linux without a GUI"

Audit (2026-09-23): **the code is already cross-platform.** The Rust crates have zero platform-specific
branches — their only macOS mentions are doc comments describing cross-platform behavior (FSEvents vs
inotify through `notify`, case-insensitive filesystems, the `/var` → `/private/var` symlink handling,
which is written defensively for both). The Tauri app already carries non-macOS menu branches
(`main.rs`), `~/.config/tau` is the XDG convention (native to Linux — if anything, macOS is the
non-standard side), icons include `.png`/`.ico` alongside `.icns`, and the rig already detects the Linux
Chrome binary. What is genuinely macOS-only today: (1) `tauri.conf.json` `bundle.targets: ["app"]` —
`.app` has no Linux equivalent, so add `appimage` (v0 distribution = direct download; `deb` is v1 apt
channel work); (2) the CI image — no webkit2gtk (why `build` skips the Linux bundle; the fix is Tauri's
documented prerequisites: `libwebkit2gtk-4.1-dev` + `libgtk-3-dev`); (3) the launch smoke (leg a) —
WebKitGTK wants an X display, so Linux runs it under `xvfb-run`; (4) `dev_bridge.rs` has macos/windows
branches with Linux falling through — a dev-only tool, left alone. Work: the bundle target, the CI apt
deps + xvfb, a Linux leg-a, and a spec §13 note that "Linux second" includes the GUI.

### G. The demo rig becomes a real-app E2E suite (the "demo" retires)

What "demo" is today: a second Vite entry (`app/svelte/demo.ts` + `app/demo.html`) that runs the real
Svelte frontend in a plain browser with **Tauri IPC mocked** (`@tauri-apps/api/mocks` `mockIPC`) — the
mock answers `tau_command` from a **JS-generated** 10k-entry fixture (`lib/fixture.ts`) and
**JS-generated** 25 ms streams. No Rust core, no Tauri, no real session file: a frontend-only harness
wearing a "demo" name (leftover of #29, which stripped the demo *mode* but kept the *name*).

User decision (2026-09-23): the primary GUI test is **the real app running on a real session file**, not a
mocked-IPC demo:
- **One 10k-entry session file, written by the real `SessionStore`** (the generator already exists in
  `crates/tau-core/tests/large_session.rs`), shared by the Rust tests, the Rust `snapshot.rs` test, *and*
  the GUI E2E — this kills the triplicated-fixture drift (gap G6) at the source: one artifact in the real
  on-disk format instead of three hand-kept twins.
- **The E2E launches the real Tauri binary** pointed at a test workspace containing that file, asserts on
  the DOM, and drives two deterministic 25 ms streams through the real core via a **dev-gated canned
  provider** (the core's existing `canned()` test seam promoted to a dev-build-only `canned://` provider —
  a documented test hook, not product surface). A debug build is enough for correctness; the perf bar
  keeps its current fidelity (measured on a dev build, as today).
- **Driver**: the in-tree `dev_bridge` — the vendored copy of the tauri-agent-tools bridge (an HTTP server
  that lets a tool evaluate JS in the running webview; the npm package itself is dead dep B6, and
  `agent-eval.mjs` is its in-repo replacement). It is **debug-build-only** (`#[cfg(debug_assertions)]` in
  `main.rs`), so the E2E runs against a debug build: fine for correctness, and the spec §8 bar measures the
  DOM side (a debug core coalesces at the same 25 ms). If a release-fidelity number ever matters, the fix is
  to env-gate the bridge for release (a documented test mode) or take the official route —
  tauri-plugin-wdio + `@wdio/tauri-service` (decided at implementation).
- **What the mock rig uniquely gave, and where it goes:** the ability to feed the frontend *adversarial*
  event sequences the real core never emits (the `652b3a6` twin-identity class). That coverage moves to
  the store-level Vitest suite (1c), where `mockIPC` is the right tool at the store layer. The `demo.ts` /
  `demo.html` / `fixture.ts` demo surface retires; the ~35 rig checks port to the real-app E2E where they
  apply.
- Wire `app/package.json` `"test:frontend"` → the E2E runner; CI runs it in the app job (macOS) and the
  linux-acceptance job (Linux, post-F2). README + spec §8 point at the suite, not a demo.

### H. File-size policy in `AGENTS.md`

Add to the Coding-conventions section: **every code file stays under a soft 500-LOC limit and a hard
1000-LOC limit.** The soft limit is a review smell that invites a split; the hard limit is a blocker, and
per the repo's "enforced in CI where mechanical" rule a small CI check fails on any file over 1000 LOC
(the currently-over-limit files are grandfathered until the C1/C2 split lands). This is the standing rule
that makes the C1/C2 work *required* rather than optional: `app/src-tauri/src/core.rs` is 6,297 LOC — 6× the
hard limit — and must be broken up as part of moving the harness.

---

## Phase 1 — testing

Baseline: 265 Rust tests (native `#[test]`/`tokio::test`, zero mocking libraries), the demo rig (headless
Chrome over CDP, zero deps) is the GUI's de-facto regression harness, and the Tauri boundary itself is the
only untested seam. Full detail + a ready-to-paste CI YAML in `01-testing.md`.

### 1a. Runner + CI shape (½–1 d)

1. CI: add `taiki-e/install-action@v2` (`tool: nextest@0.9.146`), replace `cargo test --workspace` with
   `cargo nextest run --workspace`. **Why**: per-test process isolation turns a documented failure mode — a
   pooled keep-alive connection keeping the tokio runtime alive (ticket #23, noted in three places) — from
   "wedges the whole suite" into "one red test". No test-code changes.
2. `scripts/acceptance.sh`: accept an optional **leg-filter argument** (`./scripts/acceptance.sh a e f`; no
   args = all, preserving current behavior).
3. Restructure `ci.yml` to four jobs, driven by `just` (item F): **rust** (macos+linux matrix:
   fmt/clippy/`nextest`), **frontend** (ubuntu: `just frontend` + the store suite of 1c), **app**
   (macos **and** linux: `just build` + **launch smoke via `just acceptance a`** — today the bundle is
   built but never launched in CI, a launch-panic would pass; the Linux leg lands with F2, under
   `xvfb-run`), and **linux-acceptance** (ubuntu: `just build` + leg e + the real-app E2E suite of item G
   as separate steps so a red suite can't mask a red core leg). Live legs (b/c/d) stay local — no `TAU_LIVE`
   in CI.

### 1b. Rust additions (1–2 d)

4. Add `proptest = "1.11"` (workspace dep + `tau-core` dev-dep). **ADOPT** — the one genuinely
   property-shaped module is `hashline.rs` (62³ anchor allocation, collision probing, survivor reuse; the
   spec §5.4 contract is invariant-shaped, not case-shaped) and it has only 11 hand-picked tests. Add 4–6
   property tests: unique anchor per distinct canon line; determinism; survivor reuse across `apply_edit`;
   stale/reversed/malformed rejection; `render → apply_edit → content` round-trip.
5. One SSE property test in `provider.rs`: arbitrary chunk boundaries over a valid body decode to the same
   events (generalizes the UTF-8-split case).
6. Move `tau_command` from `main.rs` into `lib.rs` (the only code move; it's the crate's public transport
   surface, and a `tests/` target can't see binary items). `main.rs` keeps `generate_handler!`.
7. `app/src-tauri/Cargo.toml`: add `tauri = { version = "2.11", features = ["test"] }` as a **dev-dependency**;
   new `tests/tauri_smoke.rs` with one `tauri::test::mock_app()` test calling `tau_command` directly with
   managed state — pinning the `CoreState` TypeId wiring, the `spawn_blocking` hop, and the `JoinError →
   ProtocolError` mapping (the one real Tauri surface, G4). **Caveat**: Tauri marks `tauri::test` *unstable* —
   pin the exact tauri minor (workspace pins 2.11) and keep it to one test.

**Tauri best-practice verdict:** tau already follows the recommended shape more than most apps — Tauri
appears only in `main.rs` (a 6-line `tau_command` over `Core::dispatch`), and all 51 `core.rs` tests exercise
the whole stateful surface without any Tauri object. "Move the tests to `mock_app`" is a non-migration; the
one smoke test above is the best-practice-correct completion. **Frontend: no new mocking layer** — `demo.ts`
already uses the official `mockIPC` + `shouldMockEvents` in a real browser, the strongest form of the pattern.
There is no `tauri-mocks` npm package (404); the official module is the one already in use.

### 1c. Svelte store suite (1 d) — minimal adoption

**Verdict: a Vitest suite for `store.svelte` only.** No component tests, no WebdriverIO, no browser farm.

- The demo rig already covers the frontend in a stronger form than jsdom component tests could (real browser,
  real entry, DOM geometry the rig asserts on); `@testing-library/svelte` would test rendering internals the
  rig already beats — dead abstraction.
- The one gap a rig can't close is speed + locality for the store: `store.svelte` (1,106 lines — boot,
  `applyEvents`, lane queueing, session switch, refetch waves) is diagnosed today from a FAIL line in a 1,124-line
  CDP script. A Node-side Vitest suite runs in ms and names the failing function.

Concrete: `app/package.json` (`"test": "vitest run"`, devDeps `vitest@^5.0.1`, `jsdom@^30.1.1`);
`app/vite.config.ts` `test: { environment: 'jsdom', include: ['svelte/**/*.svelte.test.ts'] }`;
`app/svelte/lib/store.svelte.test.ts` — import `./store.svelte`, `mockIPC` reusing `fixture.ts` data, assert
boot-from-snapshot, `applyEvents` (stream_start/delta/end + usage), lane queueing (force/steering/follow-up),
session switch + archive convergence, the `skill_list_changed` guard, the `file_tree_changed` refetch wave.
~15–30 tests, each mirroring an existing rig check. CI: `npm run test` in the frontend job (node — no Chrome).

### Explicitly NOT doing (v0) — each with the trigger that would change it

- **insta** — until a ticket needs to pin *evolving* output upstream can't pin (the ported-OM fidelity test
  already diffs byte-for-byte against pinned `third_party/mastra-om/`, a better oracle than `.snap`).
- **criterion** — until the perf bars flake on CI; and then the bar moves to the demo rig first, not a bench harness.
- **mockall / wiremock / httpmock** — the hand-rolled seams (`canned*` providers, `mock_server`) are
  domain-meaningful and cover the streamed behaviors (mid-body EOF) these libraries can't express; revisit when
  the post-v0 chat-completions fallback multiplies the HTTP surface.
- **trybuild** — until a crate has external consumers (`tau serve`).
- **`@testing-library/svelte` component tests** — the rig covers rendering in a real browser; jsdom can't.
- **WebdriverIO + `tauri-plugin-wdio`** — until a ticket needs to drive the real webview's native surface
  (menus/dialogs), which the rig can't see.
- **Live legs (b/c/d) in CI** — until there's a CI-usable endpoint + secrets plumbing (an infra ticket, not a testing one).

---

## Phase 2 — architecture

From `03-code.md`. Candidates use the codebase-design vocabulary; each has a strength badge. C3 is the first
ticket; C5/C6 are same-week companions; **C1 is the strategic decision**; C4/C7 are deferred.

### C3 (first) — extract the store's entry/twin merge into a pure module — **Strong**

`store.svelte` owns state + the ~350-line event switch + the paged-read **twin merge** (`mergeHydrated`/`isTwin`,
L737–860) + panes + the `window.__tau` rig seam. The twin logic is the heart of render correctness (the
`652b3a6` class of regressions) and is inlined in `applyEvents`, keyed on id-namespace rules no type exposes —
its only test surface is the browser rig, so a one-line twin change costs a full rig build+run.

Extract a pure `lib/entries.ts`: `applyStreamEvent`, `applyToolEvent`, `mergeHydrated(entries, live, views)`
with `isTwin` inside (functions over `Entry`/`ViewEntry` arrays, no `$state`). The store keeps state and calls
in; the `__tau` seam can feed the pure functions directly, shrinking the rig. *Locality*: the streamed-id vs
file-counter identity rules concentrate in one place. *Test impact*: twin regressions become unit-testable
without a browser (pairs with 1c's store suite). No wire change, no ADR surface.

### C5 — wire the config knobs or delete them — **Strong**

`config.rs` parses and unit-tests three knobs that **no production path consumes**: `Gui.coalesce_ms`
(`core.rs:596` hard-codes its own 25), `Gui.reasoning_visible` (never read — the GUI always shows reasoning),
and `Sessions.blob_threshold_bytes` (`with_blob_threshold` is test-only; `workspace_config` never applies
`[sessions]`). `session.rs:237–238` even has a doc comment asserting the missing wiring. That's the "flags for
nonexistent features" the v0 rules forbid.

Spec §12/§3 promise the surface, so the spec-conformant answer is to **wire** each knob to its consumer
(builder reads `config.gui.coalesce_ms`; the store/`EntryCard` consult `reasoning_visible`;
`SessionStore::for_workspace` takes the threshold from the workspace config) — **or** delete the fields, their
tests, and the false doc line (the v0-minimal answer). *Decision*: wire (spec-conformant) vs delete (minimal).
Wiring also makes B's `with_blob_threshold` item a keep.

### C6 — (see Phase 0 C) mirror fix + diff check — **Strong** — already in Phase 0.

### C1 (decided) — move the app-side `Core` to the right side of the core↔Tauri seam — **Strong, strategic**

**Decided by the user (2026-09-23):** the seam is that `tau-core` is responsible for **all logic necessary
for a non-Tauri interface**, and the Tauri app holds **all Tauri-specific code** — whatever is on the wrong
side of the seam moves (`Core`'s state and transport-free logic → `tau-core`; Tauri wiring stays). Consequence
of the file-size rule (H): `app/src-tauri/src/core.rs` (6,297 lines) is 6× the 1000-LOC hard limit and must be
broken up as part of this move, not merely relocated whole. The file is the
real state owner: the live-session registry, workspace index, archive/reopen lifecycle, `run_turn` (217 lines),
the event pump, the watchers — and `dispatch` is one 900-line function. ADR-0002 exists so "a future binary
(CLI, RPC) can reuse" the standalone core, and ADR-0006 says the core is the single owner of all
session/workspace/task state — but that state lives in the Tauri *app* crate, whose name promises it's a
transport. The module header even calls itself "the thin dispatch layer".

Deletion test: delete `core.rs` and the complexity doesn't vanish — a future `tau serve`/CLI must re-implement
(or link the Tauri crate to get) the whole session lifecycle. The complexity is real; it's behind the wrong
seam. **Solution**: move the transport-free composition into `tau-core` (a `harness` module: `Core` state,
`dispatch`, `run_turn`, `pump`, watchers, workspace index). The Tauri shell keeps only `main.rs`
(menu, `tau_command`, `emit`) + the debug `dev_bridge`. The 3,100 test lines in `core.rs` (which construct
`Core` directly, no window) move as-is. *Reinforces* ADR-0002/0006 — no conflict. It's large; scope it as its
own effort, not a v0 ticket.

### C2 — collapse the live-vs-disk session duality (follows C1) — **Strong**

"session is a file, and may also be live" is resolved independently by eight `dispatch` arms, each branching
live→in-memory vs closed→`SessionStore`, plus the app parsing the JSONL header through two private structs
(`DiskHeader`, `ArchHeader`) that duplicate `session::Header`, plus hand-rolled `.tau/sessions`/`archive/`
walks. ADR-0005 makes the format a core concern; today it's a second interface the app keeps in sync by hand.
Give `tau-core::session` the listing surface it implies (`list_workspace(cwd) -> Vec<SessionMeta>` including
`archive/`), one `session_access` seam the app calls once per command; the duplicate header structs delete.
*Collapses naturally once C1 lands* — sequence after it.

### C4 — extract the transcript virtualization math — **Worth exploring (defer)**

The most bug-dense area of the git log (mid-scroll yank `3036dc3`, Svelte-Map reactivity `e866549`, boot
churn `19dde1f`, pin semantics, `652b3a6`) is one component whose invariants are defined against
`performance.now()` and DOM scroll events, interleaved with `$derived`/`$effect`, so the window/pin math can't
be unit-tested. Extracting the pure math (entries, heights map, scroll top/height, viewport → window bounds,
offset, pin transitions on a classified input event) concentrates the verified invariants where they can be
asserted; the component becomes DOM glue. The code is currently **stable** — the cost shows up as rig cycles
per future scroll regression. Defer; do it if scroll regressions recur.

### C7 — `SessionFork` / `SessionBranch`: two variants, one behaviour — **Speculative (defer)**

The protocol's own doc says "fork is a branch, not a new session"; dispatch pattern-matches them together
(`core.rs:2073`). No v0 producer. Spec §8 lists both, so this is a surface decision, not dead code. When the
first producer lands, land it as one variant; keep both only if the fork/branch distinction earns a behavioural
difference. Pre-v1 is breakable; no ADR conflict.

---

## Suggested ticket graph

```
Phase 0 (independent, land in any order)
  A comments   B dead code   C mirror-fix   D ADRs+agent-docs   E prototypes+tracker
  F build/acceptance -> justfile (just)   F2 Linux GUI support
  G demo rig -> real-app E2E suite   H file-size policy in AGENTS.md

Phase 1 (ordered)
  1a runner+CI ── 1b rust (proptest, tauri smoke) ── 1c svelte store suite (home of the adversarial-sequence checks)

Phase 2
  C3 entries-module ── 1c's store suite benefits from it
  C5 wire-or-delete config
  C1 harness move (DECIDED, strategic) ── C2 session-access (collapses; together they break core.rs to <1000 LOC)
  C4 (defer)   C7 (defer)
  orphaned #28–#33: left as standalone tickets (user decision — no map)
```

## What "done" looks like for v0

- Zero "what" comments / banners / dead code in the scanned surfaces (Phase 0).
- No throwaway prototypes tracked; no redundant branches; #33 closed; missing labels + junk edge fixed;
  orphaned #28–#33 left as standalone tickets (no map charted).
- One build/acceptance entry point (a `just` justfile) replaces `./build` + `./scripts/acceptance.sh`; the two shell
  scripts are deleted; all work delegated to cargo/npm/vite/tauri.
- Linux has a GUI: the Linux bundle target + CI webkit2gtk/xvfb land (F2); the launch smoke runs on both
  platforms.
- The frontend E2E is **the real app on a real 10k-entry session file** (G) — one shared fixture for Rust
  and GUI tests; the mocked-IPC "demo" surface retires, its adversarial-sequence checks live in the store
  suite.
- `AGENTS.md` carries the file-size rule (soft 500 / hard 1000 LOC) and no code file exceeds the hard limit —
  `core.rs` is split as part of C1/C2.
- CI: rust (matrix, nextest), frontend (build + store suite), app (macOS + Linux build + launch smoke),
  linux-acceptance (leg e + the real-app E2E). `tauri::test` smoke pins the one real Tauri seam.
- The store's twin-merge logic is a pure, unit-tested module; the config surface is true (wired or gone).

No open decisions remain — C1 is decided, the orphaned tickets are left, and every item in this roadmap is
mechanical and ticket-ready.
