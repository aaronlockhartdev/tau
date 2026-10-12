<script lang="ts">
  // The command palette (workbench P6, ⌘K): one filter box over grouped
  // actions — open a session, follow a crew member, fire an app action.
  // j/k + arrows move the active row, Enter runs it, Esc closes.
  import { store, switchSession, setFollow, newSession, archiveSession, stop } from '../lib/store.svelte';
  import { paletteItems, filterItems, type PaletteItem } from '../lib/palette';

  const open = $derived(store.paletteOpen);
  let query = $state('');
  let active = $state(0);
  let input: HTMLInputElement | undefined = $state(undefined);

  const items = $derived.by(() => {
    const all = paletteItems({
      sessions: store.sessions,
      current: store.current,
      focus: store.focus,
      openSession: (sid) => {
        store.paletteOpen = false;
        void switchSession(sid);
      },
      follow: (h) => {
        store.paletteOpen = false;
        setFollow(h);
      },
      newSession: () => {
        store.paletteOpen = false;
        const ws = store.current ? store.sessions[store.current!]?.meta.workspace : null;
        if (ws) void newSession(ws);
      },
      openAttention: () => (store.attentionOpen = true),
      openFleet: () => (store.fleetOpen = true),
      toggleFocus: () => (store.focus = !store.focus),
      stop: () => void stop(),
      archive: () => {
        if (store.current) void archiveSession(store.current);
      }
    });
    const filtered = filterItems(all, query);
    return filtered;
  });

  // Keep the active row in range as the filter narrows the list.
  $effect(() => {
    if (active >= items.length) active = Math.max(0, items.length - 1);
  });

  // Refocus the box whenever the palette opens.
  $effect(() => {
    if (open) {
      query = '';
      active = 0;
      void input?.focus();
    }
  });

  function move(delta: number): void {
    const n = items.length;
    if (n === 0) return;
    active = (active + delta + n) % n;
  }

  function run(item: PaletteItem | undefined): void {
    if (item) item.run();
  }

  function onKey(e: KeyboardEvent): void {
    // The palette owns the keys while it is open; the app-level map must
    // not see them.
    e.stopPropagation();
    if (e.key === 'ArrowDown' || e.key === 'j') move(1);
    else if (e.key === 'ArrowUp' || e.key === 'k') move(-1);
    else if (e.key === 'Enter') run(items[active]);
    else if (e.key === 'Escape') store.paletteOpen = false;
  }
</script>

{#if open}
  <div class="dim" role="presentation" onclick={() => (store.paletteOpen = false)}></div>
  <div class="pal" role="dialog" aria-label="Command palette">
    <input
      bind:this={input}
      bind:value={query}
      class="q"
      placeholder="jump to a session, crew member, or action…"
      aria-label="palette filter"
      onkeydown={onKey}
    />
    <div class="list">
      {#each items as item, i (i)}
        {#if i === 0 || items[i - 1]!.group !== item.group}
          <div class="glabel">{item.group}</div>
        {/if}
        <button
          class="row"
          class:on={i === active}
          onclick={() => run(item)}
          onmousemove={() => (active = i)}
        >
          <span class="label">{item.label}</span>
          {#if item.hint}<span class="hint">{item.hint}</span>{/if}
        </button>
      {/each}
      {#if items.length === 0}
        <div class="empty">no match</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .dim {
    position: fixed;
    inset: 0;
    background: rgba(5, 6, 8, 0.72);
    z-index: 60;
  }
  .pal {
    position: fixed;
    top: 18%;
    left: 50%;
    transform: translateX(-50%);
    width: 520px;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: var(--raised);
    border: 1px solid var(--line2);
    border-radius: 10px;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.55);
    z-index: 61;
    overflow: hidden;
  }
  .q {
    flex: none;
    width: 100%;
    box-sizing: border-box;
    padding: 12px 14px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--line);
    color: var(--tx);
    font: 13px var(--sans);
    outline: none;
  }
  .list {
    overflow: auto;
    min-height: 0;
    padding: 4px 0;
  }
  .glabel {
    padding: 6px 14px 2px;
    font: 700 9.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--faint);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    box-sizing: border-box;
    padding: 6px 14px;
    background: none;
    border: none;
    text-align: left;
    cursor: pointer;
    color: var(--tx);
    font-size: 12.5px;
  }
  .row.on {
    background: color-mix(in srgb, var(--acc) 14%, transparent);
  }
  .hint {
    font: 10.5px var(--mono);
    color: var(--faint);
  }
  .empty {
    padding: 14px;
    font-size: 12px;
    color: var(--faint);
  }
</style>
