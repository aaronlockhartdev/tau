<script module lang="ts">
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
  const INPUT_TTL = 200;
  const pending = new Map<string, number>();
  let settleTimer = 0;
  function armSettle() {
    if (settleTimer) return;
    settleTimer = setTimeout(() => {
      settleTimer = 0;
      if (performance.now() - inputIntent <= INPUT_TTL) return armSettle();
      for (const [k, h] of pending) if (heights.get(k) !== h) heights.set(k, h);
      if (pending.size) {
        pending.clear();
        heightsGen.gen++;
      }
    }, INPUT_TTL + 60);
  }
  function reportMeasurement(key: string, h: number) {
    if (heights.get(key) === h) return;
    if (performance.now() - inputIntent <= INPUT_TTL) {
      pending.set(key, h);
      armSettle();
    } else {
      heights.set(key, h);
      heightsGen.gen++;
    }
  }
</script>

<script lang="ts">
  // Virtualized transcript — the prototype's approach, faithfully:
  // measured per-entry heights (measured on mount, cached, unmount
  // off-screen), total = sum of measured + estimated, the DOM windowed to
  // viewport + buffer, positioned by an absolutely-positioned inner track.

  import { onMount, onDestroy } from 'svelte';
  import EntryCard from './EntryCard.svelte';
  import { store, fetchWindow } from '../lib/store.svelte';
  import type { Entry } from '../lib/protocol';

  const BUFFER = 600;
  // A viewport within this of the measured bottom counts as "at the bottom".
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
    // estimate: ~1.45 line-height per line + card padding; live streams grow.
    const lines = Math.max(1, Math.ceil((e.text ?? '').length / 72)) + (reasoning ? 2 : 0);
    return lines * 21 + 36;
  }

  const total = $derived.by(() => {
    void heightsGen.gen;
    return all.reduce((sum, e) => sum + hOf(e), 0);
  });

  function computeWin() {
    void heightsGen.gen;
    const t0 = performance.now();
    const { top, h } = scroll;
    const view = h || 600;
    let acc = 0;
    let start = 0;
    for (let i = 0; i < all.length; i++) {
      const hh = hOf(all[i]);
      if (acc + hh >= top) {
        start = i;
        break;
      }
      acc += hh;
      start = i + 1;
    }
    let end = start;
    acc = 0;
    for (let i = 0; i < all.length; i++) {
      acc += hOf(all[i]);
      if (acc >= top + view + BUFFER) {
        end = Math.max(start, i);
        break;
      }
      end = i + 1;
    }
    // The slice sits at its natural document position (the prototype's
    // prefix[start]); the viewport scroll does the rest. Adding scrollTop
    // to it double-counts the scroll — at mid-session the slice lands ~S
    // px below the viewport and the screen is blank.
    return {
      start,
      end: Math.min(all.length, end + 1),
      offset: scrollBefore(start),
      ms: performance.now() - t0
    };
  }
  const win = $derived(computeWin());

  // The status bar's render stats (spec §9 seg3): the window's range and
  // the compute cost, reported on every recompute.
  $effect(() => {
    void win.start;
    void win.end;
    store.renderMs = win.ms;
    store.renderRange = `${win.start + 1}–${win.end} of ${all.length}`;
  });

  function scrollBefore(i: number): number {
    let acc = 0;
    for (let j = 0; j < i; j++) acc += hOf(all[j]);
    return acc;
  }

  // Scroll bookkeeping lives on the DOM, not in the reactive graph: a
  // scroll listener that writes $state from an effect re-triggers itself
  // in this Svelte. onMount registers once; the deriveds just read.
  //
  // The follow is one flag: pinned while the viewport sits within THRESHOLD
  // of the measured bottom, and only user input moves the flag — any
  // upward input (wheel up, a touch moving up, a scrollbar drag pulled up,
  // a scroll key up) unpins immediately, so a slow scroll up from the
  // bottom can never fight the catch-up; a downward arrival at the bottom
  // re-pins. The input-intent flag stamps the last user scroll input and
  // the scroll handler consults it for both directions. That gate is what
  // keeps boot churn (the estimate→measured correction clamps the viewport
  // up with no input behind it) and the follow's own catch-up (its
  // scrollTop write fires a downward scroll event the old unconditional
  // re-pin read as a user arrival) from flipping the follow.
  onMount(() => {
    const node = el;
    if (!node) return;
    // Input intent: the last user scroll input (wheel, touch, scrollbar
    // drag, scroll key), time-stamped in the module's inputIntent (shared
    // with reportMeasurement). The unpin branch consumes it; a re-pin
    // leaves it live (a wheel up within the TTL unpins in its listener
    // regardless). The TTL expires an arm the scroll never spent.
    const onWheel = (e: WheelEvent) => {
      inputIntent = performance.now();
      if (e.deltaY < 0) pinned = false;
    };
    let touchY = 0;
    const onTouchStart = (e: TouchEvent) => {
      touchY = e.touches[0]?.clientY ?? 0;
    };
    const onTouchMove = (e: TouchEvent) => {
      const y = e.touches[0]?.clientY ?? 0;
      inputIntent = performance.now();
      if (y < touchY) pinned = false;
      touchY = y;
    };
    // A scrollbar drag fires no wheel/touch event; a pointerdown on the
    // element itself is the drag (content clicks target their own nodes).
    let dragY: number | null = null;
    const onPointerDown = (e: PointerEvent) => {
      if (e.target === node) {
        inputIntent = performance.now();
        dragY = e.clientY;
      }
    };
    // A native scrollbar drag may fire no pointermove; the flag armed at
    // pointerdown plus the scroll handler then carries the movement. Where
    // moves do arrive, refresh the flag so a long drag's TTL never expires
    // mid-drag.
    const onPointerMove = (e: PointerEvent) => {
      if (dragY === null) return;
      if (e.buttons & 1) inputIntent = performance.now();
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
      if (e.key === 'ArrowUp' || e.key === 'PageUp' || e.key === 'Home') pinned = false;
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
    const onScroll = () => {
      scroll.top = node.scrollTop;
      scroll.h = node.clientHeight;
      const dist = node.scrollHeight - node.scrollTop - node.clientHeight;
      const intent = performance.now() - inputIntent <= INPUT_TTL;
      if (node.scrollTop > lastTop) {
        if (dist <= THRESHOLD && intent) pinned = true;
      } else if (node.scrollTop < lastTop) {
        if (intent) {
          inputIntent = 0;
          pinned = false;
        }
      }
      lastTop = node.scrollTop;
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
        const top = node.scrollHeight - node.clientHeight;
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
    };
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
      el.scrollTop = Math.max(0, total - el.clientHeight);
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
  $effect(() => {
    const start = win.start;
    const end = win.end;
    const c = cur;
    if (c) void fetchWindow(c, start, end - start);
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
    <div class="track" style="height: {total}px">
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
</style>
