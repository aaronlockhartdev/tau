<script lang="ts">
  // Virtualized transcript on virtua: the library owns windowing and
  // per-item measurement; this file carries only the blessed stick-to-bottom
  // pattern (virtua's Svelte Chat example) plus one extension — the pin
  // effect also tracks the last card's measured height, so a streaming reply
  // follows the viewport while its height grows without a new item landing.
  // Paged hydration (spec §8) is driven from virtua's visible range via
  // findItemIndex over the scroll offset.
  import { onDestroy, onMount, untrack } from 'svelte';
  import { Virtualizer, type VirtualizerHandle } from 'virtua/svelte';
  import EntryCard from './EntryCard.svelte';
  import { store, fetchWindow } from '../lib/store.svelte';
  import type { Entry } from '../lib/protocol';

  // Must match .scroll's padding-top: the padding sits before the track in
  // scroll coordinates, so the pin target adds it back to land flush on the
  // last card (virtua's coordinates start at the track).
  const TOP_PAD = 14;
  // Sub-pixel tolerance at fractional devicePixelRatio (the Chat story's).
  const STICK_TOLERANCE = -1.5;
  // A session opens pinned at its tail: the first page is the tail.
  const OPEN_TAIL = 20;
  const FETCH_MARGIN = 5; // ≈ one 600px buffer of 120px cards
  const BUFFER = 600;

  const cur = $derived(store.current);
  // This mount's session. A plain const, not the derived: at destroy the
  // derived re-reads store.current (already the next session), which made
  // the prune below a dead branch.
  const mountedFor = store.current;

  let el = $state<HTMLDivElement | null>(null);
  // The --tsgo checker cannot infer the exported handle of virtua's generic
  // .svelte component from node_modules (it sees a plain SvelteComponent),
  // so the binding is cast at the site. `any` is the only form that passes:
  // the check runs both ways, and `unknown` and the handle type each fail
  // one direction. Every use of `ref` below stays fully typed through
  // VirtualizerHandle.
  let ref = $state<VirtualizerHandle | undefined>(undefined);
  let shouldStickToBottom = $state(true);
  // Last reported scroll offset: detects a user scroll up (offset dropping) so
  // the pin can release instead of fighting a mid-read against an expanding
  // card.
  let lastScrollOffset = 0;

  const entries = $derived(cur ? store.sessions[cur].entries : []);
  const live = $derived(cur ? store.sessions[cur].live : []);
  // The pre-first-output window: a turn is dispatched but no stream/tool
  // event has landed yet. The animated dots sit at the tail of the track.
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
  type Row = (typeof all)[number];

  // The cards' self-reported heights keyed by entry id. The pin effect reads
  // the last entry's row, so a streaming card's growth re-pins even when no
  // new item lands. Off-screen cards unmount but their entries stay: the
  // map is bounded by the session's entries and dies with the mount.
  const cardHeights = $state<Record<string, number>>({});

  // Pin (the Chat pattern + the streaming extension): re-runs when the
  // rendered set changes (a new item, a hydration, a stream delta — `all`
  // is a fresh array on each), when the last card's measured height
  // changes, or when the stick flag re-arms. A scrollTo to the current
  // position is a browser no-op, so an already-at-bottom pin is a fixed
  // point.
  $effect(() => {
    if (!ref) return;
    const n = all.length;
    if (n === 0) return;
    const lastId = all[n - 1].id;
    void cardHeights[lastId];
    if (!shouldStickToBottom) return;
    ref.scrollToIndex(n - 1, { align: 'end', offset: TOP_PAD });
  });

  // The turn's ellipses appeared (a new turn, pre-first-output): jump to the
  // very bottom so they're visible. They sit below the last card — past the
  // virtualized items — so a direct DOM scroll, not scrollToIndex.
  $effect(() => {
    if (awaiting && el) el.scrollTop = el.scrollHeight;
  });

  // Paged read around the visible window (spec §8): virtua's range drives
  // fetchWindow. Hysteresis: a scroll burst moves the window a little each
  // frame, and re-fetching on every move pipelines IPC (each page also
  // costs an O(n) merge) — only fetch when the range has drifted beyond the
  // last fetch by ~a buffer.
  const lastFetched = new Map<string, { start: number; end: number }>();
  // Ranges hydrated while a turn was in flight. A mid-turn read can capture a
  // streamed assistant before it is finalized (empty), and the page is only
  // re-read on scroll, so it stays stale while the user watches the tail. The
  // turn-end effect below re-reads the union to heal those entries.
  const duringTurn = new Map<string, { start: number; end: number }>();
  function onVirtuaScroll(offset: number): void {
    if (!ref) return;
    // A user scroll up (offset dropping past a small threshold) releases the
    // pin: a rapidly expanding card otherwise re-arms the stick and yanks the
    // view back to the tail mid-read.
    const scrolledUp = offset < lastScrollOffset - 4;
    lastScrollOffset = offset;
    shouldStickToBottom =
      !scrolledUp &&
      offset - ref.getScrollSize() + ref.getViewportSize() >= STICK_TOLERANCE;
    const c = cur;
    const n = all.length;
    if (!c || n === 0) return;
    const start = Math.max(0, ref.findItemIndex(offset));
    const end = Math.min(n - 1, ref.findItemIndex(offset + ref.getViewportSize()));
    const last = lastFetched.get(c);
    if (last && start >= last.start - FETCH_MARGIN && end <= last.end + FETCH_MARGIN) return;
    lastFetched.set(c, { start, end });
    store.renderRange = `${start}–${end} of ${n}`;
    void fetchWindow(c, start, end - start);
    if (store.sessions[c]?.turn !== 'idle') {
      const d = duringTurn.get(c) ?? { start: 0, end: 0 };
      duringTurn.set(c, { start: Math.min(d.start, start), end: Math.max(d.end, end) });
    }
  }

  // A session opens pinned at its tail, so the first page is the tail.
  // One-shot per session — `all` is read untracked or every stream delta
  // would re-issue it.
  $effect(() => {
    const c = cur;
    if (!c) return;
    const n = untrack(() => all.length);
    const count = Math.min(OPEN_TAIL, n);
    lastFetched.set(c, { start: n - count, end: n });
    void fetchWindow(c, n - count, count);
    if (store.sessions[c]?.turn !== 'idle') {
      const d = duringTurn.get(c) ?? { start: 0, end: 0 };
      duringTurn.set(c, { start: Math.min(d.start, n - count), end: Math.max(d.end, n) });
    }
  });

  // A turn just ended: re-read the ranges hydrated while it ran, directly
  // (bypassing the scroll hysteresis). Entries finalized since the in-flight
  // read — a streamed assistant captured empty before its turn ended — are now
  // final, so the re-read heals the empty-then-filled cards.
  $effect(() => {
    const c = cur;
    if (!c) return;
    const t = store.sessions[c]?.turn;
    if (t !== 'idle') return;
    const r = duringTurn.get(c);
    if (!r) return;
    duringTurn.delete(c);
    void fetchWindow(c, r.start, r.end - r.start);
  });

  // A new send re-arms the pin (explicitly: the jump to the fresh user
  // entry is the pin effect's own work).
  $effect(() => {
    void store.tailJump;
    shouldStickToBottom = true;
  });

  onMount(() => {
    // The boot pin is a direct DOM write: the virtua driver observes the
    // scroll element a tick after mount, so a handle scroll scheduled before
    // that would race it; the DOM write always lands.
    if (el) el.scrollTop = el.scrollHeight;
  });

  onDestroy(() => {
    // A closed workspace drops its sessions from the store — that is when
    // this session's card expansion state goes with it (a plain session
    // switch keeps the row, so the state persists across remounts).
    if (mountedFor !== null && store.current !== mountedFor && !store.sessions[mountedFor]) {
      for (const k of [...store.entryOpen.keys()]) if (k.startsWith(`${mountedFor}:`)) store.entryOpen.delete(k);
    }
  });

  // V2 turn headers: 'you' on the user's own messages, 'agent' on the first
  // entry of a response block.
  function turnLabel(idx: number): 'you' | 'agent' | '' {
    const e = all[idx];
    if (e.kind === 'user' && !e.source && !e.skill) return 'you';
    const prev = all[idx - 1];
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
    <div class="track">
      <Virtualizer
        bind:this={ref as any}
        scrollRef={el ?? undefined}
        data={all}
        getKey={(d: Row) => d.id}
        bufferSize={BUFFER}
        onscroll={onVirtuaScroll}
      >
        {#snippet children(e: Row, idx: number)}
          {@const hk = cur ? `${cur}:${e.id}` : e.id}
          <EntryCard
            entry={e}
            heightKey={hk}
            report={(h) => {
              if (cardHeights[e.id] !== h) cardHeights[e.id] = h;
            }}
            sourceLabel={sourceLabelFor(e)}
            parentLabel={parentLabel}
            turn={turnLabel(idx)}
          />
        {/snippet}
      </Virtualizer>
      {#if awaiting}
        <div class="waiting">
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
    /* The browser's native scroll anchoring fights the virtualizer. */
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
  .waiting {
    padding: 16px 16px 12px;
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
