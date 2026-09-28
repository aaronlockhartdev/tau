<script module lang="ts">
  import type { Entry } from '../lib/protocol';
  // key: `${session}:${entryId}` — heights persist across remounts. A plain
  // Map: Svelte 5.57 does not deep-proxy Maps, so the map never tracks on
  // its own; gen is the reactive trigger — a changed measurement bumps it,
  // and the total/window deriveds read it and re-sum.
  const heights = new Map<string, number>();
  const heightsGen = $state({ gen: 0 });

  // Last user scroll input (wheel, touch, scrollbar drag, scroll key),
  // performance.now()-stamped by Transcript's listeners. While input is
  // live, new measurements are held in `pending` instead of the map: the
  // slice is an absolutely-positioned transform, so a mid-scroll
  // estimate→measured correction — in the total or in the window's
  // re-basing prefix — re-offsets the slice and yanks the visible content
  // under the cursor. At settle the pending heights flush as one clean
  // re-layout. (A deferred map *write* is not enough: the window's
  // scrollBefore reads the map directly, so a changed value in the map
  // jumps the prefix the moment the window boundary crosses it.)
  let inputIntent = 0;
  // Direction of the last user scroll input: -1 up, 0 none known (a
  // pointerdown before the drag moves), 1 down. A downward (or
  // directionless) stamp must never authorize a re-pin.
  let inputDir = 0;
  const INPUT_TTL = 200;
  const pending = new Map<string, number>();
  let settleTimer = 0;
  // Scroll anchoring (B3's residual): a flush that changes the combined
  // height of entries above the viewport shrinks the track, and the browser
  // clamps scrollTop — the view then ratchets up on its own. So at flush
  // time, shift scrollTop by the anchor's prefix delta, keeping the entry at
  // the top of the viewport where it was.
  let anchorEl: HTMLElement | null = null;
  let anchorAll: Entry[] = [];
  let anchorHOf: (e: Entry) => number = () => 0;
  let anchorPinned = true;
  // The component's scroll state, mirrored here so a flush can write the
  // compensated scrollTop into it in the SAME synchronous block as the height
  // change. The window then computes once with (new heights, new scrollTop)
  // consistently — not one frame on a stale top (the jump/flicker).
  let anchorScroll: { top: number; h: number } | null = null;
  export function registerAnchor(
    el: HTMLElement,
    all: Entry[],
    hOf: (e: Entry) => number,
    scroll: { top: number; h: number }
  ): void {
    anchorEl = el;
    anchorAll = all;
    anchorHOf = hOf;
    anchorScroll = scroll;
  }
  export function setAnchorPinned(p: boolean): void {
    anchorPinned = p;
  }
  function flushMeasurements(batch: Array<[string, number]>): void {
    const el = anchorEl;
    if (!el) return;
    const oldTop = el.scrollTop;
    // The anchor is the entry containing oldTop, under the OLD heights
    // (first walk) — then the new heights land, and the second walk sums the
    // new prefix at the same anchor.
    let acc = 0;
    let anchorIdx = anchorAll.length;
    let anchorPrefixOld = 0;
    for (let i = 0; i < anchorAll.length; i++) {
      const oh = anchorHOf(anchorAll[i]);
      if (acc + oh > oldTop) {
        anchorIdx = i;
        anchorPrefixOld = acc;
        break;
      }
      acc += oh;
    }
    let wrote = false;
    for (const [k, h] of batch) {
      if (heights.get(k) !== h) {
        heights.set(k, h);
        wrote = true;
      }
    }
    if (!wrote) return;
    let prefixNew = 0;
    for (let i = 0; i < anchorIdx; i++) prefixNew += anchorHOf(anchorAll[i]);
    heightsGen.gen++;
    // A pinned view is owned by the bottom snap; compensating it would fight
    // that snap every flush.
    if (!anchorPinned) {
      const delta = prefixNew - anchorPrefixOld;
      if (delta) {
        // Clamp to the track: a flush that shrinks it (measured < estimate)
        // must not push scrollTop past the bottom (an unsolicited nudge).
        el.scrollTop = Math.min(oldTop + delta, el.scrollHeight - el.clientHeight);
      }
    }
    // Mirror the (possibly compensated) scrollTop into the reactive scroll
    // state NOW, in the same block as the height change (see anchorScroll).
    if (anchorScroll) {
      anchorScroll.top = el.scrollTop;
      anchorScroll.h = el.clientHeight;
    }
  }
  function armSettle() {
    if (settleTimer) return;
    settleTimer = setTimeout(() => {
      settleTimer = 0;
      if (performance.now() - inputIntent <= INPUT_TTL) return armSettle();
      if (pending.size) {
        flushMeasurements([...pending]);
        pending.clear();
      }
    }, INPUT_TTL + 60);
  }
  function reportMeasurement(key: string, h: number) {
    if (heights.get(key) === h) return;
    if (performance.now() - inputIntent <= INPUT_TTL) {
      pending.set(key, h);
      armSettle();
    } else {
      flushMeasurements([[key, h]]);
    }
  }
</script>

<script lang="ts">
  // Virtualized transcript — the prototype's approach, faithfully:
  // measured per-entry heights (measured on mount, cached, unmount
  // off-screen), total = sum of measured + estimated, the DOM windowed to
  // viewport + buffer, positioned by an absolutely-positioned inner track.
  //
  // Windowing is prefix-sum based: a cumulative-height array (rebuilt only on
  // a height change, never on a scroll) makes every window query a binary
  // search + O(1) offset read instead of three O(n) walks of the list.

  import { onDestroy, onMount, untrack } from 'svelte';
  import EntryCard from './EntryCard.svelte';
  import { store, fetchWindow } from '../lib/store.svelte';
  import { buildPrefix, computeWin } from '../lib/virtualizer';

  const BUFFER = 600;
  // A viewport within this of the measured bottom counts as "at the bottom".
  // It is a floor: the live threshold widens to the bottom card's height
  // estimate error (a 120px tool card measures 100–140px, so a genuine
  // wheel-to-bottom can land 10–50px short and must still re-arm the pin).
  const THRESHOLD = 8;

  const cur = $derived(store.current);
  // This mount's session. A plain const, not the derived: at destroy the
  // derived re-reads store.current (already the next session), which made
  // the prune below a dead branch.
  const mountedFor = store.current;

  let el = $state<HTMLDivElement | null>(null);
  let scroll = $state({ top: 0, h: 0 });
  // Plain, not reactive: the scroll handler and the send effect own it;
  // no derived may read it.
  let pinned = true;
  const entries = $derived(cur ? store.sessions[cur].entries : []);
  const live = $derived(cur ? store.sessions[cur].live : []);
  // The pre-first-output window: a turn is dispatched but no stream/tool event
  // has landed yet. The animated dots sit at the tail of the track here.
  const awaiting = $derived(cur ? store.sessions[cur].turn === 'starting' : false);
  // A sub-agent notification's label: the child session's own title.
  const sourceLabelFor = (e: Entry): string =>
    e.kind === 'user' ? (e.source ? store.sessions[e.source]?.meta.title ?? '' : '') : '';
  // The parent → child direction: a user entry inside a child session is
  // the parent's message (task assignment, steering, follow-up).
  const parentLabel = $derived.by(() => {
    const p = cur ? store.sessions[cur]?.meta.parent : null;
    return p ? store.sessions[p]?.meta.title ?? '' : '';
  });

  // Live entries sit at the tail of the virtual list (the running message is
  // the most recent thing in the session).
  const all = $derived([
    ...entries,
    ...live.map((l) => ({
      id: l.id,
      kind: 'message' as const,
      text: l.text,
      reasoning: l.reasoning,
      usage: undefined
    }))
  ]);

  function hOf(e: Entry): number {
    // A fully empty entry renders nothing (EntryCard's hasContent).
    const reasoning = e.kind === 'message' || e.kind === 'interrupted' ? e.reasoning : undefined;
    if (
      !((e.text ?? '').trim() ||
        reasoning ||
        e.kind === 'interrupted' ||
        (e.kind === 'tool' && (e.status === 'running' || Boolean(e.args) || Boolean(e.output))))
    )
      return 0;
    if (cur) {
      const m = heights.get(`${cur}:${e.id}`);
      if (m !== undefined) return m;
    }
    // Tool cards carry no text to size from; they measure 100–140px, so
    // estimate flat. The old text-length estimate (57px) made every height
    // flush rebase the viewport by the undercount (B3).
    if (e.kind === 'tool') return 120;
    // estimate: ~1.45 line-height per line + card padding; live streams grow.
    const lines = Math.max(1, Math.ceil((e.text ?? '').length / 72)) + (reasoning ? 2 : 0);
    return lines * 21 + 36;
  }

  // Prefix-sum windowing. The entries half is the bulk and is stable across a
  // turn (it changes only on a hydration or a measurement flush); the live
  // half is a few streaming cards. Both rebuild only when heightsGen bumps or
  // their list changes — never on a scroll.
  const entriesHeights = $derived.by(() => {
    void heightsGen.gen;
    return entries.map(hOf);
  });
  const liveHeights = $derived.by(() => {
    void heightsGen.gen;
    return live.map(hOf);
  });
  const prefix = $derived.by(() => buildPrefix(entriesHeights.concat(liveHeights)));
  const total = $derived.by(() => prefix[prefix.length - 1]);

  // The windowed slice: a binary search over the prefix (O(log n)) + an O(1)
  // offset read, recomputed only when the prefix, the list length, or the
  // scroll position changes — never an O(n) walk per frame.
  const win = $derived(
    computeWin(prefix, entries.length + live.length, scroll.top, scroll.h || 600, BUFFER)
  );

  // Scroll bookkeeping lives on the DOM, not in the reactive graph: a
  // scroll listener that writes $state from an effect re-triggers itself
  // in this Svelte. onMount registers once; the deriveds just read.
  //
  // The follow is one flag: pinned while the viewport sits within the
  // (estimate-aware) threshold of the measured bottom, and only user input
  // moves the flag — any upward input (wheel up, a touch moving up, a
  // scrollbar drag pulled up, a scroll key up) unpins immediately, so a slow
  // scroll up from the bottom can never fight the catch-up; a downward
  // arrival at the bottom re-pins — a live DOWNWARD intent authorizes it,
  // never an upward one (the catch-up write itself is a downward movement).
  // The input-intent flag stamps the last user scroll input with its
  // direction, and the scroll handler consults it for both directions. That
  // gate is what keeps boot churn and the follow's own catch-up from
  // flipping the follow.
  onMount(() => {
    const node = el;
    if (!node) return;
    // Cached scrollHeight (a forced-layout read): refreshed on a height flush
    // and in the follow loop, so the per-frame scroll handler reads the cache
    // instead of forcing layout.
    let scrollH = 0;
    // A downward arrival that lands within the (estimate-aware) threshold of
    // the bottom re-arms the pin. The bottom card's unmeasured estimate is
    // the dominant source of bottom-position error, so scale the threshold to
    // it rather than a fixed 8px that a 100–140px tool card overshoots.
    const bottomThreshold = (): number => {
      const nn = all.length;
      if (nn === 0) return THRESHOLD;
      const e = all[nn - 1];
      const measured = cur ? heights.has(`${cur}:${e.id}`) : true;
      return Math.max(THRESHOLD, measured ? 0 : 2 * hOf(e));
    };
    // Input intent: the last user scroll input (wheel, touch, scrollbar
    // drag, scroll key), time-stamped in the module's inputIntent (shared
    // with reportMeasurement). The unpin branch consumes it; a re-pin
    // leaves it live (a wheel up within the TTL unpins in its listener
    // regardless). The TTL expires an arm the scroll never spent.
    const onWheel = (e: WheelEvent) => {
      inputIntent = performance.now();
      inputDir = e.deltaY < 0 ? -1 : 1;
      if (e.deltaY < 0) pinned = false;
    };
    let touchY = 0;
    const onTouchStart = (e: TouchEvent) => {
      touchY = e.touches[0]?.clientY ?? 0;
    };
    const onTouchMove = (e: TouchEvent) => {
      const y = e.touches[0]?.clientY ?? 0;
      inputIntent = performance.now();
      // A finger dragging DOWN pulls the content up (away from the tail):
      // that is an upward scroll — unpin. A finger dragging UP pushes the
      // content down (toward the tail): a downward scroll — never unpin.
      // (The wheel handler already has this direction right.)
      if (y > touchY) {
        inputDir = -1;
        pinned = false;
      } else if (y < touchY) {
        inputDir = 1;
      }
      touchY = y;
    };
    // A scrollbar drag fires no wheel/touch event; a pointerdown on the
    // element itself is the drag (content clicks target their own nodes).
    let dragY: number | null = null;
    const onPointerDown = (e: PointerEvent) => {
      if (e.target === node) {
        inputIntent = performance.now();
        inputDir = 0;
        dragY = e.clientY;
      }
    };
    // A native scrollbar drag may fire no pointermove; the flag armed at
    // pointerdown plus the scroll handler then carries the movement. Where
    // moves do arrive, refresh the flag so a long drag's TTL never expires
    // mid-drag.
    const onPointerMove = (e: PointerEvent) => {
      if (dragY === null) return;
      if (e.buttons & 1) {
        inputIntent = performance.now();
        if (e.clientY < dragY) inputDir = -1;
        else if (e.clientY > dragY) inputDir = 1;
      }
      dragY = e.clientY;
    };
    const onPointerUp = () => {
      dragY = null;
    };
    // Scroll keys are scroll input too; editable targets keep their own.
    const onKeydown = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)) return;
      inputIntent = performance.now();
      if (e.key === 'ArrowUp' || e.key === 'PageUp' || e.key === 'Home') {
        inputDir = -1;
        pinned = false;
      } else if (e.key === 'ArrowDown' || e.key === 'PageDown' || e.key === 'End') {
        inputDir = 1;
      }
    };
    // Transition classification (the virtuoso atBottom scan): compare the
    // movement against the last processed position, not the absolute
    // distance. A pending scroll event's scrollTop can be stale against a
    // bottom that has since grown (the send's jump fires after the first
    // stream growth) — re-classifying that as "scrolled away" would drop
    // the pin for the whole turn. Both directions additionally require the
    // input intent to be live: a downward move with no input behind it is
    // the follow's own catch-up write (overflow-anchor is off, so native
    // anchoring plays no part), and letting it re-pin is what made a slow
    // scroll up jitter — the pin re-armed and the catch-up snapped the
    // view back.
    let lastTop = node.scrollTop;
    // Coalesce scroll events to one state commit per frame. A 1200 px
    // wheel generates dozens of scroll events; the window recompute is now a
    // binary search, but one commit per frame still keeps the reactive graph
    // quiet.
    let scrollRaf = 0;
    const onScroll = () => {
      if (scrollRaf) return;
      scrollRaf = requestAnimationFrame(() => {
        scrollRaf = 0;
        scroll.top = node.scrollTop;
        scroll.h = node.clientHeight;
        const dist = scrollH - node.scrollTop - node.clientHeight;
        const intent = performance.now() - inputIntent <= INPUT_TTL;
        if (node.scrollTop > lastTop) {
          // Downward re-pin requires a live DOWNWARD intent: the follow's
          // own catch-up write can land right after a wheel-up stamped the
          // intent, and an upward stamp must never authorize a re-pin
          // (dogfood B3).
          if (dist <= bottomThreshold() && intent && inputDir === 1) pinned = true;
        } else if (node.scrollTop < lastTop) {
          if (intent) {
            inputIntent = 0;
            inputDir = 0;
            pinned = false;
          }
        }
        lastTop = node.scrollTop;
      });
    };
    onScroll();
    node.addEventListener('scroll', onScroll, { passive: true });
    node.addEventListener('wheel', onWheel, { capture: true, passive: true });
    node.addEventListener('pointerdown', onPointerDown, { capture: true, passive: true });
    node.addEventListener('touchstart', onTouchStart, { capture: true, passive: true });
    node.addEventListener('touchmove', onTouchMove, { capture: true, passive: true });
    node.addEventListener('pointermove', onPointerMove, { capture: true, passive: true });
    node.addEventListener('pointerup', onPointerUp, { capture: true, passive: true });
    document.addEventListener('keydown', onKeydown, { capture: true, passive: true });
    node.scrollTo({ top: node.scrollHeight });
    // While pinned, one catch-up per frame to the measured bottom, and only
    // when behind (rAF runs after the flush that re-laid the track, so a
    // growth and its catch-up land in the same frame — one monotonic step,
    // no up/down fight, no smooth-scroll mix). The target is the node's
    // scrollHeight — the rendered slice's bottom — so a live stream's text
    // growth (which stretches the card without a measurement flush) is
    // followed frame by frame. The measured-total snap below is the other
    // half of the pin: it lands the window at the session's true bottom in
    // the same flush the heights change, which the rAF loop alone only
    // ratchets toward one rendered window at a time.
    let raf = 0;
    const follow = () => {
      if (pinned) {
        scrollH = node.scrollHeight;
        const top = scrollH - node.clientHeight;
        if (node.scrollTop < top) node.scrollTop = top;
      }
      raf = requestAnimationFrame(follow);
    };
    raf = requestAnimationFrame(follow);
    return () => {
      node.removeEventListener('scroll', onScroll);
      node.removeEventListener('wheel', onWheel, { capture: true });
      node.removeEventListener('pointerdown', onPointerDown, { capture: true });
      node.removeEventListener('touchstart', onTouchStart, { capture: true });
      node.removeEventListener('touchmove', onTouchMove, { capture: true });
      node.removeEventListener('pointermove', onPointerMove, { capture: true });
      node.removeEventListener('pointerup', onPointerUp, { capture: true });
      document.removeEventListener('keydown', onKeydown, { capture: true });
      cancelAnimationFrame(raf);
      cancelAnimationFrame(scrollRaf);
    };
  });

  // The anchor geometry for the module's scroll compensation: the scroll
  // node, the entry list, and the effective-height function. Re-registered
  // whenever any of them changes.
  $effect(() => {
    if (el) registerAnchor(el, all, hOf, scroll);
  });
  $effect(() => {
    setAnchorPinned(pinned);
  });

  // A height flush re-lays the track; the module mirrors the compensated
  // scrollTop into `scroll` in the same block (registerAnchor's scroll arg),
  // so the window computes once with consistent values. The old separate
  // "sync scroll from the DOM on every flush" effect is gone for that reason.

  // Cached scrollHeight refresh on a height flush (the track height changed);
  // the follow loop also refreshes it while pinned. A clean read (no write
  // follows in the effect), so it does not force a reflow.
  $effect(() => {
    void heightsGen.gen;
    if (el) {
      const node = el;
      void node.scrollHeight;
    }
  });

  // The measured-total snap: pinned and the height bookkeeping changed, so
  // the session's true bottom moved — land the viewport there in this
  // flush. The rAF catch-up alone walks one rendered window per frame, which
  // on a throttled display (xvfb) leaves the slice out of the viewport for
  // seconds between frames (a blank screen mid-walk); writing scrollTop in
  // the same flush the track grew leaves no gap. The write's own scroll
  // event recomputes the window at the new top, and the loop settles when
  // the measured bottom stops moving.
  $effect(() => {
    const t = total;
    if (pinned && el) {
      const top = t - el.clientHeight;
      if (el.scrollTop < top) el.scrollTop = top;
    }
  });

  onDestroy(() => {
    // A closed workspace drops its sessions from the store — that is when
    // this session's cached heights go with it (a plain session switch
    // keeps the row, so the heights persist across remounts).
    if (mountedFor !== null && store.current !== mountedFor && !store.sessions[mountedFor]) {
      for (const k of [...heights.keys()]) if (k.startsWith(`${mountedFor}:`)) heights.delete(k);
      for (const k of [...pending.keys()]) if (k.startsWith(`${mountedFor}:`)) pending.delete(k);
      for (const k of [...store.entryOpen.keys()]) if (k.startsWith(`${mountedFor}:`)) store.entryOpen.delete(k);
    }
  });

  // Opening a session starts at its tail, not where the previous
  // session's viewport was: the pin survives a switch (it is dropped by
  // scrolling up, never reset), and the old scrollTop can sit far past a
  // shorter session's bottom. Reset the pin and land on the new bottom;
  // the measured-total snap below keeps it there as heights flush.
  $effect(() => {
    const c = cur;
    if (c && el) {
      pinned = true;
      // untrack: this effect must stay one-shot per session. If it tracked
      // total, every height flush would re-run it, re-arm the pin, and snap
      // a user who scrolled up back to the bottom (B3's second re-pin path).
      el.scrollTop = Math.max(0, untrack(() => total) - el.clientHeight);
    }
  });

  // A new send jumps the view to the fresh user entry and re-arms the pin
  // (explicitly: a no-op jump fires no scroll event), so the turn is
  // followed as it is written.
  $effect(() => {
    void store.tailJump;
    if (el) {
      el.scrollTop = el.scrollHeight;
      pinned = true;
    }
  });

  // Paged read around the viewport (spec §8): the window's slice is
  // requested from the core; the response replaces the skeleton cards.
  // Hysteresis: a scroll burst moves the window a little each frame, and
  // re-fetching on every move pipelines IPC (each page also costs an O(n)
  // merge). Only fetch when the window has drifted beyond the last fetch by
  // ~a buffer, so a burst issues a few pages, not one per frame.
  const lastFetched = new Map<string, { start: number; end: number }>();
  const FETCH_MARGIN = 5; // ≈ one 600px buffer of 120px cards
  $effect(() => {
    const start = win.start;
    const end = win.end;
    const c = cur;
    if (!c) return;
    const last = lastFetched.get(c);
    if (last && start >= last.start - FETCH_MARGIN && end <= last.end + FETCH_MARGIN) return;
    lastFetched.set(c, { start, end });
    void fetchWindow(c, start, end - start);
  });

  // V2 turn headers: 'you' on the user's own messages, 'agent' on the first
  // entry of a response block. The label renders inside the card's measured
  // wrap, so the virtualized height math stays exact.
  function turnLabel(idx: number): 'you' | 'agent' | '' {
    const i = win.start + idx;
    const e = all[i];
    if (e.kind === 'user' && !e.source && !e.skill) return 'you';
    const prev = all[i - 1];
    return prev && prev.kind === 'user' ? 'agent' : '';
  }
</script>

<div class="scroll" bind:this={el} class:empty={all.length === 0}>
  {#if all.length === 0}
    <div class="ph">
      <div>no entries yet</div>
      <div class="sub">send a message below to start the session</div>
    </div>
  {:else}
    <div class="track" style="height: {total + (awaiting ? 44 : 0)}px">
      <div class="inner" style="transform: translateY({win.offset}px)">
        {#each all.slice(win.start, win.end) as e, idx (e.id)}
          {@const hk = cur ? `${cur}:${e.id}` : e.id}
          <EntryCard
            entry={e}
            heightKey={hk}
            report={(h) => reportMeasurement(hk, h)}
            sourceLabel={sourceLabelFor(e)}
            parentLabel={parentLabel}
            turn={turnLabel(idx)}
          />
        {/each}
      </div>
      {#if awaiting}
        <div class="waiting" style="top: {total}px">
          <span class="dots"><i></i><i></i><i></i></span>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .scroll {
    flex: 1;
    overflow-y: auto;
    overflow-anchor: none;
    min-height: 0;
    position: relative;
    padding-top: 14px;
  }
  .scroll.empty {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .ph {
    text-align: center;
    color: var(--dim);
    font-size: 14px;
  }
  .ph .sub {
    margin-top: 6px;
    font-size: 12px;
    color: var(--faint);
  }
  .track {
    position: relative;
    width: 100%;
  }
  .inner {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
  }
  .waiting {
    position: absolute;
    left: 0;
    right: 0;
    padding: 28px 16px 14px;
  }
  .dots {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
  .dots i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dim);
    animation: dotpulse 1s infinite ease-in-out;
  }
  .dots i:nth-child(2) {
    animation-delay: 0.15s;
  }
  .dots i:nth-child(3) {
    animation-delay: 0.3s;
  }
  @keyframes dotpulse {
    0%,
    80%,
    100% {
      opacity: 0.25;
      transform: translateY(0);
    }
    40% {
      opacity: 1;
      transform: translateY(-3px);
    }
  }
</style>
