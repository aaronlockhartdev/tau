# Research: How to disable the WKWebView elastic (rubber-band) scroll bounce on macOS in a Tauri v2 app

Researched 2026-10-10. All claims are cited to primary sources fetched during this task: WebKit
source (github.com/WebKit/WebKit, `main` branch plus pinned commits), wry source (github.com/tauri-apps/wry,
`main` + tag `wry-v0.57.0` — the exact version in `Cargo.lock`), Tauri source (github.com/tauri-apps/tauri,
`main`), GitHub issue/PR metadata via the `gh` API, and source files of third-party apps.
Secondary write-ups (Stack Overflow, blog posts) were not used as evidence. Claims that could not be
verified against a primary source are marked **[unverified]**.

**Question.** Can the rubber-band/elastic overscroll bounce of `WKWebView` on macOS be disabled from a
Tauri v2 app — and if so, through what mechanism, applied where?

**Grounding (tau's setup).** Tauri 2.12.2 + wry 0.57.0 (`Cargo.lock`). The transcript is a single CSS
scroller (`.scroll`, `overflow: auto`); `overscroll-behavior: none` is computed on it; `html`/`body`
are `overflow: visible`. Despite that, a rubber band is still felt past the top/bottom edge with a
trackpad.

**Short answer.** **Partially.** There is no *public* macOS API to disable the bounce, and wry/Tauri
expose no option. But WebKit ships a stable **private** API — `-[WKWebView _setRubberBandingEnabled:]`
(macOS 10.13.4+, present in current WebKit) — that is the root-level knob, and Tauri v2 already hands us
the raw `WKWebView` pointer (`Webview::inner()`), so it can be called from a Rust command with **no wry
fork**. Its one hard limitation: it only governs the **main frame** (the document root), not inner
scrollable elements — and tau's only scroller is an inner element. Current WebKit additionally gates
inner-scroller rubber-banding on the scroller's own `overscroll-behavior`, which tau already sets — so
the felt bounce's origin is version-sensitive and needs a 5-minute empirical check before committing to
a fix (see §7). The JS wheel-interceptor remains the only lever that covers inner scrollers on older
WebKit, with the tradeoffs in §6.

---

## 1. Apple public API: there is none

- **`WKWebView.scrollView` does not exist on macOS at all.** In WebKit's *public* header the property
  is compile-gated to iOS:
  `Source/WebKit/UIProcess/API/Cocoa/WKWebView.h:489-492`:
  ```c
  #if TARGET_OS_IPHONE
  @property (nonatomic, readonly, strong) UIScrollView *scrollView;
  ```
  ([WebKit `main`, WKWebView.h](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/API/Cocoa/WKWebView.h)).
  This is why every `webView.scrollView.bounces = NO` recipe (including the one proposed in wry issue
  #557) cannot even compile on macOS. Apple's own docs page for `scrollView`
  ([developer.apple.com](https://developer.apple.com/documentation/webkit/wkwebview/1614784-scrollview))
  lists iOS/iPadOS/Catalyst availability only — consistent with the header.
- **`WKWebViewConfiguration` / `WKPreferences`: no bounce keys.** The private companion headers
  `WKWebViewConfigurationPrivate.h` and `WKPreferencesPrivate.h` contain zero bounce/rubber-band
  symbols (fetched from WebKit `main`, 0 grep hits).
- **Conclusion:** on macOS the bounce is not configurable through any public WebKit API. It is a
  WebProcess-internal behavior of the remote scrolling tree (see §2.3 for where it actually lives).

## 2. WebKit internals: where the bounce lives, and the one knob that exists

### 2.1 The private API: `_setRubberBandingEnabled:` (confirmed, current)

- Declared in WebKit's private header
  [`Source/WebKit/UIProcess/API/Cocoa/WKWebViewPrivate.h:880`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/API/Cocoa/WKWebViewPrivate.h):
  ```objc
  @property (nonatomic, setter=_setRubberBandingEnabled:) _WKRectEdge _rubberBandingEnabled
      WK_API_AVAILABLE(macos(10.13.4));
  ```
- The argument type, from
  [`Source/WebKit/UIProcess/API/Cocoa/_WKRectEdge.h`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/API/Cocoa/_WKRectEdge.h):
  ```objc
  typedef NS_OPTIONS(NSUInteger, _WKRectEdge) {
      _WKRectEdgeNone = 0,
      _WKRectEdgeLeft = 1 << CGRectMinXEdge,
      _WKRectEdgeTop  = 1 << CGRectMinYEdge,
      _WKRectEdgeRight = 1 << CGRectMaxXEdge,
      _WKRectEdgeBottom = 1 << CGRectMaxYEdge,
      _WKRectEdgeAll  = Left | Top | Right | Bottom,   // == 15, WebKit's default
  } WK_API_AVAILABLE(macos(10.13.4), ios(18.0), visionos(2.0));
  ```
- Implemented for macOS in
  [`Source/WebKit/UIProcess/API/mac/WKWebViewMac.mm:1711-1714`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/API/mac/WKWebViewMac.mm):
  ```objc
  - (void)_setRubberBandingEnabled:(_WKRectEdge)state
  {
      _impl->setRubberBandingEnabled(state);
  }
  ```
  → [`Source/WebKit/UIProcess/mac/WebViewImpl.mm:7052-7054`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/mac/WebViewImpl.mm):
  `m_page->setRubberBandableEdges(toRectEdges(state))`.

  The `WK_API_AVAILABLE(macos(10.13.4))` annotation is the primary-source claim that this has shipped
  in the macOS WebKit framework since 10.13.4 (Catalina era).

### 2.2 The legacy recipe `_setBouncesOnOverflow:` is dead

The widely-circulated fix `[[webView _setBouncesOnOverflow:NO]` (e.g. the Stack Overflow thread
referenced in the wry PR #558 discussion, and the assumption in issue #557) **no longer exists**:
a code search of `WebKit/WebKit` `main` returns **0** hits for `bouncesOnOverflow`.
Its historical presence in the old `Source/WebKit/UIProcess/legacy/WKWebView.mm` is **plausible but
[unverified]** here (Apple-era tag fetches 404'd), but the absence in current WebKit is confirmed.
Do not build on it.

### 2.3 Where the bounce is actually decided (why a UI-process knob works)

On macOS, scrolling is handled by the WebProcess's `ScrollingTree` (remote/async scrolling); the
UIProcess only forwards the client's rubber-banding permission:

- `WebPageProxy::setRubberBandableEdges` —
  [`Source/WebKit/UIProcess/WebPageProxy.h:1706`](https://github.com/WebKit/WebKit/blob/main/Source/WebKit/UIProcess/WebPageProxy.h).
- WebProcess side: `ScrollingTree::setClientAllowedMainFrameRubberBandableEdges` —
  [`Source/WebCore/page/scrolling/ScrollingTree.cpp:1027-1032`](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/page/scrolling/ScrollingTree.cpp).
- Defaults: all edges `RubberBandingBehavior::Always` and all edges pinned —
  [`ScrollingTree.h:400-401`](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/page/scrolling/ScrollingTree.h)
  (`enum class RubberBandingBehavior { Always, Never, BasedOnSize }` is in
  [`Source/WebCore/platform/ScrollTypes.h:163-167`](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/platform/ScrollTypes.h)).
- The gate is consulted in
  [`ScrollingTreeScrollingNode::shouldRubberBandOnSide`](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/page/scrolling/ScrollingTreeScrollingNode.cpp)
  (current `main`, lines ~158-173):
  ```cpp
  auto mainFrameRubberBandingBehavior =
      scrollingTree()->clientAllowsMainFrameRubberBandingOnSide(side);
  if (isRootNode() && mainFrameRubberBandingBehavior == RubberBandingBehavior::Never)
      return false;                       // <-- the private API acts HERE
  switch (side) {
  case BoxSide::Top:
  case BoxSide::Bottom:
      if (!overscrollBehaviorAllowsVerticalRubberBand())
          return false;                   // <-- inner scrollers are gated by CSS
      ...
  ```
  with `overscrollBehaviorAllowsVerticalRubberBand()` defined as
  `verticalOverscrollBehavior != OverscrollBehavior::None`
  ([`ScrollingTreeScrollingNode.h:211`](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/page/scrolling/ScrollingTreeScrollingNode.h)).

  Two consequences, both primary-source:
  1. `_setRubberBandingEnabled:` only short-circuits the **root node** (`isRootNode()`).
     **Inner scrollers are not affected by it.**
  2. An inner scroller with `overscroll-behavior: none` does **not** rubber-band, in current WebKit.

  Caveat on the wheel path: `shouldRubberBand(wheelEvent, targeting)` (same file, lines ~209-231)
  returns `true` unconditionally for a *latched* node (`isLatchedNode()`) or
  `EventTargeting::NodeOnly` — i.e. once a gesture has latched, or for node-targeted events, the
  per-side checks are bypassed. This is a plausible (but **[unverified]** for tau's exact gesture)
  reason `overscroll-behavior: none` can still leak a bounce in some WebKit builds.

### 2.4 `overscroll-behavior` on macOS WebKit: when it started mattering

- Commit [`9051fcabe10c1f0383b8c403d9425e6699188041`](https://github.com/WebKit/WebKit/commit/9051fcabe10c1f0383b8c403d9425e6699188041)
  (2022-01-29, [bug 220139](https://bugs.webkit.org/show_bug.cgi?id=220139), "Implement CSS
  overscroll-behavior for asynchronous scroll on Mac"): "Add function for blocking scroll chaining and
  filtering scroll delta depending on the values of overscroll behavior". The 2022-era
  `shouldRubberBand` at that commit already contained an
  `overscrollBehaviorAllowsRubberBand()` clause (verified by fetching
  `ScrollingTreeScrollingNode.cpp` at that SHA, line 132).
- The tighter *per-side* gate (2.3) is a later refinement (part of the
  `ScrollableAreaParameters` refactor; exact introducing commit not pinned down here — **[unverified]**).
- **Practical reading:** WebKit from the Safari 16 era (macOS 12.3/13+) onward honors
  `overscroll-behavior` in the Mac trackpad path. If tau's bounce persists with the CSS applied,
  the running WebKit is older than that, or the gesture takes a latched/NodeOnly path (§2.3 caveat).

## 3. wry (0.57.0 — the version in tau's `Cargo.lock`)

- **No macOS bounce option.** The only bounce handling in wry is **iOS-only** —
  `src/wkwebview/mod.rs` at tag [`wry-v0.57.0`](https://github.com/tauri-apps/wry/blob/wry-v0.57.0/src/wkwebview/mod.rs)
  (lines 525-531, inside `#[cfg(target_os = "ios")]`):
  ```rust
  // disable scroll bounce by default
  // https://developer.apple.com/documentation/webkit/wkwebview/1614784-scrollview?language=objc
  let scroll_view: Retained<UIScrollView> = objc2::msg_send![&webview, scrollView];
  scroll_view.setBounces(false)
  ```
  (Same shape in `main` — `src/wkwebview/mod.rs:526-531`.) No `bounce`/`overscroll` attribute exists
  for macOS (repo-wide grep: only the iOS block and a `underPageBackgroundColor` overscroll-*color*
  mention).
- **PR #558** ([tauri-apps/wry#558](https://github.com/tauri-apps/wry/pull/558), "fix: disalbe bounce
  option for macos, closes #557") added a `bounce: bool` attribute and, when false, registered a
  **no-op `scrollWheel:` override on the WKWebView subclass** — i.e. it swallowed *all* scroll-wheel
  events. The author closed it unmerged (2022-04-25) stating "This PR's solution works in specific
  situations", after maintainer wusyong pointed out "WkWebview on macOS doesn't have scroll view"
  (citing the same Apple-docs iOS-only point as §1). It was never superseded by a merged wry option.
- **Issue #557** ([tauri-apps/wry#557](https://github.com/tauri-apps/wry/issues/557), "Set scroll
  bounce on WKWebView") is the only bounce issue in the wry tracker (searched 2026-10-10:
  "bounce" → just #557; "overscroll OR elastic OR rubber" → none). Closed 2023-05-17 with no option
  added. **No newer PR/issue exists as of this research.**
- **Relevant precedent:** wry already calls *private* WebKit APIs from Rust on macOS — e.g.
  `src/wkwebview/mod.rs` (0.57.0) line 990: "NOTE: Private API — `drawsBackground` is a private KVC
  key on WKWebView instance", plus `developerExtrasEnabled` on `WKPreferences` and
  `underPageBackgroundColor` (macOS 12+). A `_setRubberBandingEnabled:` call in a wry fork would be
  consistent with existing wry practice. The WKWebView object is created at
  `InnerWebView::new` (`src/wkwebview/mod.rs` ~line 302, `WryWebView::alloc(mtm).set_ivars(...)`),
  which is the natural insertion point for such a fork.

## 4. Tauri (2.12.2)

- **No config key, capability, or builder option** for the bounce. Both tracked issues were closed
  with CSS advice and no option implemented:
  - [tauri-apps/tauri#4309](https://github.com/tauri-apps/tauri/issues/4309) "[feat] [MacOS]
    Disabled scroll rubber banding" — maintainer summary (wusyong, 2022-09-06): "macOS doesn't have
    scrollView. It's only supported on iOS … So on macOS, we can only use workaround above [CSS]."
  - [tauri-apps/tauri#4802](https://github.com/tauri-apps/tauri/issues/4802) "Add config to disable
    the view sliding movement" — closed 2022-07-31 as duplicate of #4309.
- **The hook Tauri already gives us:** `Webview::inner()` returns the raw `WKWebView` pointer on
  macOS — `crates/tauri/src/webview/mod.rs:199-203` (main):
  ```rust
  /// Returns the [WKWebView] handle.
  #[cfg(any(target_os = "macos", target_os = "ios"))]
  pub fn inner(&self) -> *mut std::ffi::c_void { self.0.webview }
  ```
  reached from a command via `AppHandle::get_webview(label)`
  (`crates/tauri/src/lib.rs:591`). No `macOSPrivateApi` gate applies to it: that config key is
  documented as "**No-op in Tauri 2.12.1+ because the APIs are always enabled now**"
  (`crates/tauri-utils/src/config.rs:3371-3375`).
- So from tau's Rust side the call is a plain `objc2::msg_send!` on that pointer — **no wry fork, no
  Tauri config change**.

## 5. Prior art: who disables the bounce today, and how

- **Nerda** (macOS browser, [kamafozilov/nerda.browser](https://github.com/kamafozilov/nerda.browser)):
  `Sources/Nerda/Tab.swift:293-297` — at page creation:
  ```swift
  // Scrolled to its end, the page stays put, as in Chrome: WebKit's
  // rubber band pulls it on, over a blank ground. No edge bounces.
  // WebKit SPI (`_WKRectEdge`): should it go, pages bounce again.
  Self.set(page, "_setRubberBandingEnabled:", UInt(0))
  ```
  and its decision log `docs/decisions.md` (line 45) states the scope limit in their own words:
  "Page edges: a page scrolled to its end stays put, as in Chrome, through WebKit SPI
  (`_setRubberBandingEnabled:`, as each page is made); … **A box that scrolls inside a page (a chat,
  a code view) still bounces: WebKit gives no switch for those.**"
- **Nook** ([nook-browser/Nook](https://github.com/nook-browser/Nook)):
  `Nook/Utils/WebKit/FocusableWKWebView.swift:107-122` — toggles the edge mask *dynamically* inside a
  `scrollWheel(with:)` override (`15` = `_WKRectEdgeAll` = WebKit's default; `15 & ~backEdge` while
  a back/forward gesture is active), with a defensive `responds(to:)` guard before the
  `NSSelectorFromString("_setRubberBandingEnabled:")` call.
- A 93-result GitHub code search for `_setRubberBandingEnabled` (2026-10-10) shows the API in active
  use across independent macOS apps and WebKit distributions (apple-oss-distributions/WebKit,
  apple-open-source/macos, CTSRD-CHERI/webkit, GPT-Computer, Search/SearchX, Lean, …).
- **Electron is not prior art here.** Current Electron on macOS renders with Chromium, not WKWebView;
  the `webPreferences.scrollBounce` option in old issues ([#5637](https://github.com/electron/electron/issues/5637),
  [#9033](https://github.com/electron/electron/issues/9033),
  [#13493](https://github.com/electron/electron/issues/13493),
  [#3170](https://github.com/electron/electron/issues/3170)) belonged to Electron's retired WebKit
  backend (pre-5.0).

## 6. JS-level fallback (characterized honestly — not recommended first)

The classic boundary interceptor: `wheel` listener on the scroller (capturing phase), compute
`scrollTop <= 0 && deltaY < 0` (and the bottom equivalent), `preventDefault()` in that case, let
native scrolling handle everything else.

Honest tradeoffs:

- **Fights the engine.** `preventDefault` on wheel in WebKit replaces native momentum with your
  handling; getting the release/decay feel right is fiddly, and the gesture feel will never match
  native.
- **Scope creep risk in a chat transcript.** A transcript scroller coexists with text selection,
  keyboard scrolling (arrow keys/space/home/end), find-in-page, and (if ever added) nested scrollable
  regions (e.g. a code block with horizontal overflow). A blanket interceptor must special-case all of
  them or it breaks them.
- **Momentum events don't exist.** Chromium's `overscroll` event (the cleaner signal) is not
  implemented in WebKit, so the only signal is raw `wheel` delta arithmetic — which is exactly what
  WebKit's own scroll tree does internally, with more information (gesture phase, latching) than a
  DOM listener can see.
- **Accessibility.** Swallowing wheel events changes behavior for trackpad users with
  "third-party mouse support", zoom, and assistive scrolling; there is no WebKit-sanctioned seam.
- It is also the only lever that reaches **inner** scrollers on **older** WebKit builds, which is the
  one scenario where it is the sole option (§7).

## 7. Recommendation for tau

### Decisive answer

**Is the macOS WKWebView bounce disableable from a Tauri v2 app today? — Partially, yes.**

- **Main frame (document root):** yes, via the private SPI `_setRubberBandingEnabled:`, callable from
  Rust through Tauri's existing `Webview::inner()` — no wry fork, no Tauri config, stable since macOS
  10.13.4, used by multiple shipping apps (§5).
- **Inner scrollable elements (tau's `.scroll` is one):** **no WebKit switch exists** — confirmed by
  the `isRootNode()` gate in WebKit source (§2.3) and by Nerda's field documentation (§5). On
  current WebKit, the *only* documented control for an inner scroller's bounce is its own
  `overscroll-behavior` (§2.3/§2.4) — which tau already sets.

### Where to apply it (if/when the SPI is the right lever)

- **Site:** `app/src-tauri/` — a macOS-gated helper module + one Tauri command, registered in
  `app/src-tauri/src/lib.rs`, shape (research sketch, not to be merged as-is):
  ```rust
  #[tauri::command]
  fn set_rubber_band(app: tauri::AppHandle, edges: u32 /* _WKRectEdge */) -> Result<(), String> {
      let Some(wv) = app.get_webview("main") else { return Ok(()) };
      #[cfg(target_os = "macos")]
      unsafe {
          let ptr = wv.inner() as *mut objc2::runtime::Object;
          // guard with `respondsToSelector:` first — private API, may vanish in a WebKit update
          objc2::msg_send![ptr, _setRubberBandingEnabled: edges];
      }
      Ok(())
  }
  ```
- **Timing:** call after webview creation (Rust `setup`/page-load hook), and **re-apply after
  main-frame navigations** — the permission is per-page state in the WebProcess
  (`ScrollingTree::setClientAllowedMainFrameRubberBandableEdges`, §2.3); Nerda re-applies per page
  load for this reason. (A 2026 WebKit commit, "Add mechanism for preserving scroll rubber banding
  across page loads" [`0d2bd8b`](https://github.com/WebKit/WebKit/commit/0d2bd8b886d3ba2d1b6e559333aa1816c9385f6),
  is consistent with this state not being preserved by default.)
- **App-Store caveat:** it is a private API. Fine for a developer tool; it would be a rejection
  vector for a MAS-distributed build. (Mitigation pattern: `respondsToSelector` guard, degrade
  gracefully — exactly what Nook does.)

### What to do first (before any code)

A 5-minute empirical disambiguation in the running dev build, because the fix depends on *which*
node is producing the felt bounce:

1. Read the WebKit version (Safari version in `navigator.userAgent`) of the machine showing the bug.
   If it predates the Safari-16-era WebKit, `overscroll-behavior` simply never gated the rubber band
   there (§2.4) — the CSS "failure" is then fully explained, and the SPI is unlikely to help an inner
   scroller anyway.
2. With the SPI applied (`_setRubberBandingEnabled:0`), scroll past the edges. Bounce gone → it was
   main-frame; ship the SPI. Bounce unchanged → it is the inner `.scroll` node, and the options are:
   upgrade the machine's macOS/WebKit (makes the existing CSS work, per §2.3), or accept the JS
   fallback (§6) as the only remaining lever.
3. Only if tau wants the fix *upstream and always-on* (e.g. as a wry `with_bounce(false)`-style
   option, correctly implemented this time via the SPI instead of PR #558's event-swallowing) is a
   wry fork/fork-PR worth considering — the insertion point is `InnerWebView::new` in wry
   `src/wkwebview/mod.rs` (§3). That is a contribution, not a local need.

### Ranked options

| # | Option | Fixes | Cost / risk |
|---|--------|-------|-------------|
| 1 | Empirical check (§7, first step) | identifies the right lever | 5 min |
| 2 | `_setRubberBandingEnabled:` via `Webview::inner()` | main-frame bounce, all macOS 10.13.4+ | private API (MAS risk), re-apply on navigation; ~30 LOC in `app/src-tauri` |
| 3 | Run on a current macOS/WebKit | inner-scroller bounce (CSS then works) | user environment, not code |
| 4 | JS wheel boundary interceptor (§6) | inner-scroller bounce on old WebKit | feel/AX/selection regressions; maintenance burden |
| 5 | wry fork / upstream PR (SPI-based) | durable, default-off option for all users | fork maintenance or upstream review; overkill for v0 |

For a v0 developer tool: do 1, then 2 if it's main-frame; otherwise 3/4 with eyes open. The
event-swallowing approach of wry PR #558 is **not** a viable option in any form.
