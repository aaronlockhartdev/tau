<script module lang="ts">
  // key: `${session}:${entryId}` — heights persist across remounts.
  export const heights = new Map<string, number>();
</script>

<script lang="ts">
  // Virtualized transcript — the prototype's approach, faithfully:
  // measured per-entry heights (measured on mount, cached, unmount
  // off-screen), total = sum of measured + estimated, the DOM windowed to
  // viewport + buffer, positioned by an absolutely-positioned inner track.

  import { onMount } from 'svelte';
  import EntryCard from './EntryCard.svelte';
  import { store, fetchWindow } from '../lib/store.svelte';
  import type { Entry } from '../lib/protocol';

  const BUFFER = 600;

  const cur = $derived(store.current);


  let el = $state<HTMLDivElement | null>(null);
  let scroll = $state({ top: 0, h: 0 });

  const entries = $derived(cur ? store.sessions[cur].entries : []);
  const live = $derived(cur ? store.sessions[cur].live : []);

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
    if (cur) {
      const m = heights.get(`${cur}:${e.id}`);
      if (m !== undefined) return m;
    }
    // estimate: ~1.45 line-height per line + card padding; live streams grow.
    const lines = Math.max(1, Math.ceil((e.text ?? '').length / 72)) + (e.reasoning ? 2 : 0);
    return lines * 21 + 36;
  }

  const total = $derived(all.reduce((sum, e) => sum + hOf(e), 0));

  function computeWin() {
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
    return { start, end: Math.min(all.length, end + 1), offset: top + scrollBefore(start) };
  }
  const win = $derived(computeWin());

  function scrollBefore(i: number): number {
    let acc = 0;
    for (let j = 0; j < i; j++) acc += hOf(all[j]);
    return acc;
  }

  // Scroll bookkeeping lives on the DOM, not in the reactive graph: a
  // scroll listener that writes $state from an effect re-triggers itself
  // in this Svelte. onMount registers once; the deriveds just read.
  onMount(() => {
    if (!el) return;
    const onScroll = () => {
      scroll.top = el.scrollTop;
      scroll.h = el.clientHeight;
    };
    onScroll();
    el.addEventListener('scroll', onScroll, { passive: true });
    el.scrollTo({ top: el.scrollHeight });
    const t = setInterval(() => {
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
      if (atBottom) el.scrollTop = el.scrollHeight;
    }, 300);
    return () => {
      el.removeEventListener('scroll', onScroll);
      clearInterval(t);
    };
  });

  // Paged read around the viewport (spec §8): the window's slice is
  // requested from the core; the response replaces the skeleton cards.
  $effect(() => {
    const start = win.start;
    const end = win.end;
    const c = cur;
    if (c) void fetchWindow(c, start, end - start);
  });
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
        {#each all.slice(win.start, win.end) as e (e.id)}
          <EntryCard entry={e} heightKey={cur ? `${cur}:${e.id}` : e.id} heights={heights} />
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
    color: #4d5462;
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
