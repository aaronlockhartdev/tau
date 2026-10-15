<script lang="ts">
  // The fleet list (workbench P6, ⌘\): a compact overlay of every session
  // across all open workspaces — state dot, name, workspace, latest line,
  // tokens. A list, deliberately: the fleet scales as rows, not a board.
  import { store, switchSession } from '../lib/store.svelte';
  import { fmtK } from '../lib/format';
  import Overlay from './Overlay.svelte';

  const open = $derived(store.fleetOpen);

  const rows = $derived.by(() => {
    const all = Object.values(store.sessions).filter((s) => !s.meta.archived);
    const rank = (s: (typeof all)[number]) =>
      s.turn === 'running' || s.subagents.some((a) => a.state === 'running') ? 0 : s.turn === 'idle' ? 1 : 2;
    return all
      .map((s) => {
        const entries = Object.values(s.entries);
        const last = entries.length > 0 ? entries[entries.length - 1] : null;
        return {
          s,
          running: rank(s) === 0,
          line: last?.text ? last.text.slice(0, 90) : ''
        };
      })
      .sort((a, b) => rank(a.s) - rank(b.s) || b.s.mru - a.s.mru);
  });

  const wsName = (id: string) => store.workspaces.find((w) => w.id === id)?.name ?? id;
</script>

{#if open}
  <Overlay
    label="Fleet"
    panel="fleet"
    z={50}
    title={`fleet · ${rows.length}`}
    onClose={() => (store.fleetOpen = false)}
  >
    <div class="list">
      {#each rows as row (row.s.meta.id)}
        <button class="row" class:run={row.running} onclick={() => { store.fleetOpen = false; void switchSession(row.s.meta.id); }}>
          <span class="dot" class:live={row.running}></span>
          <span class="name">{row.s.meta.title ?? row.s.meta.id}</span>
          <span class="ws">{wsName(row.s.meta.workspace)}</span>
          <span class="line">{row.line}</span>
          <span class="tok">{row.s.usage ? fmtK(row.s.usage.total_tokens) : ''}</span>
        </button>
      {/each}
      {#if rows.length === 0}
        <div class="empty">no sessions</div>
      {/if}
    </div>
  </Overlay>
{/if}

<style>
  :global(.fleet) {
    top: 12%;
    right: 24px;
    width: 460px;
    max-height: 70vh;
  }
  .list {
    overflow: auto;
    min-height: 0;
  }
  .row {
    display: grid;
    grid-template-columns: 10px minmax(0, 1fr) auto;
    grid-template-rows: auto auto;
    column-gap: 8px;
    align-items: baseline;
    width: 100%;
    box-sizing: border-box;
    padding: 7px 12px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--line);
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: rgba(255, 255, 255, 0.04);
  }
  .dot {
    grid-row: 1;
    grid-column: 1;
    align-self: center;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--faint);
  }
  .dot.live {
    background: var(--green);
    animation: pulse 1.2s ease-in-out infinite;
  }
  .name {
    grid-row: 1;
    grid-column: 2;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--tx);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ws {
    grid-row: 1;
    grid-column: 3;
    font: 10px var(--mono);
    color: var(--faint);
  }
  .line {
    grid-row: 2;
    grid-column: 2;
    font-size: 11px;
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tok {
    grid-row: 2;
    grid-column: 3;
    font: 10px var(--mono);
    color: var(--faint);
  }
  .empty {
    padding: 16px 12px;
    font-size: 12px;
    color: var(--faint);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
