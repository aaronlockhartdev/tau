<script lang="ts">
  // Virtualized transcript on virtua: the library owns windowing and
  // per-item measurement; this file carries only the blessed stick-to-bottom
  // pattern (virtua's Svelte Chat example). Paged hydration (spec §8) is
  // driven from virtua's visible range via findItemIndex over the scroll
  // offset.
  import { onDestroy } from 'svelte';
  import { Virtualizer, type VirtualizerHandle } from 'virtua/svelte';
  import EntryCard from './EntryCard.svelte';
  import SubagentBlock from './SubagentBlock.svelte';
  import { store, fetchWindow, currentSession, setFollow, switchSession } from '../lib/store.svelte';
  import { claimCrew, crewView, spawnRefsOf } from '../lib/subagent-view';
  import type { SubagentInfo } from '../lib/protocol';
  import {
    makeWindowingState,
    openFetches,
    scrollFetches,
    turnEndFetches,
    type TurnState
  } from '../lib/windowing';
  import type { Entry } from '../lib/protocol';

  // Sub-pixel tolerance at fractional devicePixelRatio (the Chat story's).
  const STICK_TOLERANCE = -1.5;
  const BUFFER = 600;
  // Explicit initial size for unmeasured items: opts out of virtua's
  // auto-estimation, which corrupts the total when measurements land
  // while the item set is still growing (a mid-turn open inflates the
  // total to the viewport height; the head then sits outside the render
  // window with no overflow to scroll back to it).
  const ITEM_SIZE = 40;

  const cur = $derived(store.current);
  const curSession = $derived(currentSession());
  // This mount's session. A plain const, not the derived: at destroy the
  // derived re-reads store.current (already the next session), which made
  // the prune below a dead branch.
  const mountedFor = store.current;

  let el = $state<HTMLDivElement | null>(null);
  // The virtualized content track: observed for in-place growth (below).
  let trackEl = $state<HTMLDivElement | null>(null);
  // The --tsgo checker cannot infer the exported handle of virtua's generic
  // .svelte component from node_modules (it sees a plain SvelteComponent),
  // so the binding is cast at the site. `any` is the only form that passes:
  // the check runs both ways, and `unknown` and the handle type each fail
  // one direction. Every use of `ref` below stays fully typed through
  // VirtualizerHandle.
  let ref = $state<VirtualizerHandle | undefined>(undefined);
  let shouldStickToBottom = $state(true);

  const entries = $derived(curSession ? curSession.entries : {});
  // The pre-first-output window: a turn is dispatched but no stream/tool
  // event has landed yet. The animated dots sit at the tail of the track.
  const awaiting = $derived(curSession?.turn === 'starting');
  // A sub-agent notification's label: the child session's own title.
  const sourceLabelFor = (e: Entry): string =>
    e.kind === 'user' ? (e.source ? store.sessions[e.source]?.meta.title ?? '' : '') : '';
  // The parent → child direction: a user entry inside a child session is
  // the parent's message (steering, follow-up).
  const parentLabel = $derived.by(() => {
    const p = curSession?.meta.parent ?? null;
    return p ? store.sessions[p]?.meta.title ?? '' : '';
  });

  // The transcript is the entry map's values in creation-id order
  // (ADR-0008). While a turn awaits first output a synthetic "waiting" item
  // ends the list, so the pin scrolls the ellipses into view (a
  // virtualized item, not a separate DOM node).
  const all = $derived([
    ...Object.values(entries),
    ...(awaiting ? [{ id: '__waiting__', kind: 'waiting' as const, text: '', reasoning: undefined, usage: undefined }] : [])
  ]);
  type Row = (typeof all)[number];

  // For each tool card in a run of 2+ consecutive tools, its index in the
  // run and the run's size. Rendered on the cards themselves (an indicator on
  // the first, a connector line on all) — NOT a separate item: collapsing a
  // run into one item changes the item count and stales virtua's size cache,
  // which is what produced the overlapping cards + blank gaps.
  const batchOf = $derived.by(() => {
    const m = new Map<string, { index: number; size: number }>();
    let run: string[] = [];
    const flush = () => {
      if (run.length >= 2) run.forEach((id, i) => m.set(id, { index: i, size: run.length }));
      run = [];
    };
    for (const e of all) if (e.kind === 'tool' && !isSpawn(e)) run.push(e.id); else flush();
    flush();
    return m;
  });

  // A spawn tool entry is the transcript's crew row (P2): it renders as a
  // SubagentBlock, never a batched tool chip, so a run of tools breaks at
  // it (the batchOf flush above).
  const isSpawn = (e: Row): boolean => e.kind === 'tool' && e.name === 'subagent_spawn';

  // The crew correlation (P2): spawn points claimed to the live SubagentInfo
  // list. Re-derived on any entry or subagent change; a cold session claims
  // nothing (the blocks render their static spawn marker).
  const spawnRefs = $derived(spawnRefsOf(all));
  const spawnRefOf = $derived.by(() => new Map(spawnRefs.map((r) => [r.entryId, r])));
  const crew = $derived.by(() =>
    curSession ? claimCrew(spawnRefs, curSession.subagents) : new Map<string, SubagentInfo | null>()
  );
  // The follow filter: a live crew member of THIS session, or null.
  const followInfo = $derived.by(() => {
    const h = store.follow;
    if (!h) return null;
    return curSession?.subagents.find((i) => i.handle === h) ?? null;
  });

  // New-activity pill (P2): while the user is scrolled up, arriving activity
  // (a new entry, or the tail entry's stream growing) counts up instead of
  // yanking the scroll; reaching the bottom — or the pill — clears it.
  let newCount = $state(0);
  let lastSignal = 0;
  let signalSession = '';
  const lastLen = $derived.by(() => {
    const e = all[all.length - 1];
    return e && typeof e.text === 'string' ? e.text.length : 0;
  });
  const sig = $derived(all.length * 100000 + lastLen);
  $effect(() => {
    void sig;
    void cur;
    if (cur !== signalSession) {
      signalSession = cur ?? '';
      newCount = 0;
    } else if (shouldStickToBottom) {
      newCount = 0;
    } else if (sig > lastSignal) {
      newCount++;
    }
    lastSignal = sig;
  });
  // Pin (the Chat pattern): when the rendered set changes and the stick flag
  // is set, end-align the last item. A scrollTo to the current position is a
  // browser no-op, so an already-at-bottom pin is a fixed point.
  // A tail pin is only meaningful while the content overflows the viewport:
  // a list that fits rests at the top with everything visible, and issuing
  // the pin is what leaves virtua's base state stale (the head then drops
  // out of the render window — seen on both WebKitGTK and macOS WebKit).
  $effect(() => {
    if (!ref) return;
    const n = all.length;
    if (n === 0) return;
    if (!shouldStickToBottom || !el) return;
    if (el.scrollHeight > el.clientHeight) {
      ref.scrollToIndex(n - 1, { align: 'end' });
    }
  });

  // In-place growth (a streaming assistant, a tool result landing, a card
  // expanding) leaves the entry count unchanged, so the count-driven pin
  // above never re-fires and the view loses the bottom. A ResizeObserver on
  // the track re-pins whenever the content grows while we are stuck.
  $effect(() => {
    if (!trackEl) return;
    const ro = new ResizeObserver(() => {
      if (!ref || !shouldStickToBottom) return;
      // A list that fits needs no pin: pinning it leaves virtua's base offset
      // stale — the same reason the count-pin guards on overflow.
      if (!el || el.scrollHeight <= el.clientHeight) return;
      const n = all.length;
      if (n > 0) ref.scrollToIndex(n - 1, { align: 'end' });
    });
    ro.observe(trackEl);
    return () => ro.disconnect();
  });

  // Paged read around the visible window (spec §8): virtua's range drives
  // fetchWindow through the windowing policy (lib/windowing.ts).
  // The windowing policy's bookkeeping (F1): last-fetched per session (the
  // hysteresis) and the ranges hydrated while a turn was in flight (the
  // mid-turn union the turn-end effect re-reads). Plain maps — the policy's
  // own state, not reactive.
  const win = makeWindowingState();
  // The session object each one-shot was burned on: a re-open replaces the
  // object (fresh meta-only snapshot), and the replacement loses the
  // previous object's page — the identity change is the re-open signal.
  const openedObj = new Map<string, unknown>();
  // The current session's turn state (the policy's hysteresis / union input).
  function turnOf(c: string): TurnState {
    return store.sessions[c]?.turn ?? 'idle';
  }

  function onVirtuaScroll(offset: number): void {
    if (!ref) return;
    // The blessed stick-to-bottom check: at the bottom within a sub-pixel
    // tolerance. Scrolling up drops below the tolerance and releases the pin —
    // no direction tracking.
    shouldStickToBottom =
      offset - ref.getScrollSize() + ref.getViewportSize() >= STICK_TOLERANCE;
    const c = cur;
    const n = all.length;
    if (!c || n === 0) return;
    const start = Math.max(0, ref.findItemIndex(offset));
    const end = Math.min(n - 1, ref.findItemIndex(offset + ref.getViewportSize()));
    const fetches = scrollFetches(win, c, { start, end }, turnOf(c));
    // The status bar labels the visible window on every recompute — not
    // only when a fetch fires: in a warm app (all-mode) the open-tail page
    // can already cover the boot pin, so a fetch-less recompute is the
    // norm and the label must not go stale (ticket #48).
    store.renderRange = `${start}–${end} of ${n}`;
    for (const f of fetches) void fetchWindow(c, f.start, f.count);
  }

  // A session opens pinned at its tail, so the first page is the tail.
  // `all` is read tracked so the effect re-runs when hydration lands: while
  // the session is still a stub (no entries) it issues nothing and leaves
  // the one-shot unburned — a stub's open would otherwise burn it and the
  // real tail page would never issue. The one-shot then blocks re-issue on
  // later re-fires (turn change, stream delta); when no
  // session is live it drops, and a re-open (the session object replaced)
  // drops it too, so the fresh state re-issues the tail page (#49).
  $effect(() => {
    const c = cur;
    const s = c ? store.sessions[c] : null;
    if (!c || !s) {
      win.openedFor = null;
      return;
    }
    // A re-open replaces the session object while the one-shot is burned:
    // the previous object's in-flight or applied page is lost with it, so
    // drop the one-shot (and the hysteresis record) and let openFetches
    // re-issue the tail page into the fresh state (#49).
    if (win.openedFor === c && openedObj.get(c) !== s) {
      win.openedFor = null;
      win.lastFetched.delete(c);
    }
    openedObj.set(c, s);
    const n = all.length;
    if (n === 0) return;
    for (const f of openFetches(win, c, n, turnOf(c))) void fetchWindow(c, f.start, f.count);
  });

  // A turn just ended: re-read the ranges hydrated while it ran (the policy
  // bypasses the scroll hysteresis). Entries finalized since the in-flight
  // read — a streamed assistant captured empty before its turn ended — are now
  // final, so the re-read heals the empty-then-filled cards.
  $effect(() => {
    const c = cur;
    if (!c) return;
    for (const f of turnEndFetches(win, c, turnOf(c))) void fetchWindow(c, f.start, f.count);
  });

  // A new send re-arms the pin (explicitly: the jump to the fresh user
  // entry is the pin effect's own work).
  $effect(() => {
    void store.tailJump;
    shouldStickToBottom = true;
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
    const e = all[idx]!;
    if (e.kind === 'user' && !e.source && !e.skill) return 'you';
    const prev = all[idx - 1];
    return prev && prev.kind === 'user' ? 'agent' : '';
  }
</script>

{#key cur}
  <!-- The whole scroll container is keyed per session: a surviving
       container carries the previous session's scrollTop, which a fresh
       virtua picks up as its base offset and renders a window that
       excludes the head entries. -->
  <div class="scroll" bind:this={el} class:empty={all.length === 0}>
    {#if all.length === 0}
      <div class="ph">
        <div>no entries yet</div>
        <div class="sub">send a message below to start the session</div>
      </div>
    {:else}
      <div class="track" bind:this={trackEl}>
        <Virtualizer
          bind:this={ref as any}
          scrollRef={el ?? undefined}
          data={all}
          getKey={(d: Row) => d.id}
          bufferSize={BUFFER}
          itemSize={ITEM_SIZE}
          onscroll={onVirtuaScroll}
        >
          {#snippet children(e: Row, idx: number)}
            {#if e.kind === 'waiting'}
              <div class="waiting">
                <span class="dots"><i></i><i></i><i></i></span>
              </div>
            {:else if isSpawn(e)}
              {@const info = crew.get(e.id) ?? null}
              {@const sref = spawnRefOf.get(e.id) ?? { entryId: e.id, agentType: '', contextMode: 'fresh', brief: null }}
              <div
                class="rowitem"
                class:dim={!!followInfo && info?.handle !== followInfo.handle}
              >
                <SubagentBlock
                  view={crewView(sref, info)}
                  name={
                    info
                      ? store.sessions[info.child]?.meta.title || sref.brief || sref.agentType
                      : sref.brief ?? sref.agentType
                  }
                  followed={!!followInfo && info?.handle === followInfo.handle}
                  onFollow={() =>
                    info && setFollow(followInfo?.handle === info.handle ? null : info.handle)}
                  onOpen={() => info && void switchSession(info.child)}
                />
              </div>
            {:else}
              {@const hk = cur ? `${cur}:${e.id}` : e.id}
              <div class="rowitem" class:dim={!!followInfo}>
                <EntryCard
                  entry={e}
                  heightKey={hk}
                  sourceLabel={sourceLabelFor(e)}
                  parentLabel={parentLabel}
                  turn={turnLabel(idx)}
                  batch={e.kind === 'tool' ? batchOf.get(e.id) : undefined}
                />
              </div>
            {/if}
          {/snippet}
        </Virtualizer>
      </div>
    {/if}
    {#if followInfo}
      <div class="fchip">
        following · {store.sessions[followInfo.child]?.meta.title ?? 'sub-agent'}
        <button type="button" aria-label="stop following" onclick={() => setFollow(null)}>✕</button>
      </div>
    {/if}
    {#if newCount > 0 && ref && all.length > 0}
      <button
        class="pill"
        type="button"
        onclick={() => {
          shouldStickToBottom = true;
          newCount = 0;
          if (ref) ref.scrollToIndex(all.length - 1, { align: 'end' });
        }}
      >↓ {newCount} new</button>
    {/if}
  </div>
{/key}

<style>
  .scroll {
    flex: 1;
    overflow-y: auto;
    /* The browser's native scroll anchoring fights the virtualizer. */
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
    color: var(--faint);
  }
  .track {
    position: relative;
    width: 100%;
  }
  /* virtua measures the per-item wrapper (the absolute-positioned div it
     creates per item), not the .wrap inside it. In the WebKitGTK webview the
     wrapper's first pre-layout ResizeObserver read is 0px and virtua records
     it (no >0 guard), so a 0-size item is never mounted. Flooring the observed
     element at 1px stops the 0 at the source; the real height lands on the next
     ResizeObserver tick. :global() — the wrapper is virtua's DOM, outside this
     component's scope, so a scoped rule is stripped. */
  :global(.track > div > div) {
    min-height: 1px;
  }
  .waiting {
    padding: 0 16px 12px;
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
  /* P2 item wrapper: the follow filter dims everything but the followed block. */
  .rowitem.dim {
    opacity: 0.38;
  }
  /* The follow chip: pinned at the transcript's top. */
  .fchip {
    position: absolute;
    top: 8px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 6px;
    z-index: 5;
    background: var(--raised);
    border: 1px solid var(--line2);
    border-radius: 8px;
    padding: 3px 6px 3px 10px;
    font: 11px var(--mono);
    color: var(--acc);
  }
  .fchip button {
    background: none;
    border: none;
    color: var(--dim);
    cursor: pointer;
    font: inherit;
    padding: 0 2px;
  }
  /* The new-activity pill: floats at the bottom while scrolled up. */
  .pill {
    position: absolute;
    bottom: 14px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 5;
    background: var(--raised);
    border: 1px solid var(--line2);
    border-radius: 12px;
    padding: 4px 12px;
    font: 11.5px var(--mono);
    color: var(--acc);
    cursor: pointer;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
  }
</style>
