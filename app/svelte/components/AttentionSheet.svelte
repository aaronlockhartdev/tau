<script lang="ts">
  // The attention sheet (workbench P5): the bell's overlay. A short list of
  // everything parked on the user, across all open workspaces; a jump opens
  // the session (the workspace context follows the session).
  import { store, attention, switchSession } from '../lib/store/store.svelte';
  import { fmtAgo } from '../lib/display/format';
  import Overlay from './Overlay.svelte';

  const open = $derived(store.attentionOpen);
  const items = $derived(attention());
  const wsName = (id: string) => store.workspaces.find((w) => w.id === id)?.name ?? id;

  function jump(session: string): void {
    store.attentionOpen = false;
    void switchSession(session);
  }
</script>

{#if open}
  <Overlay
    label="Needs your attention"
    panel="sheet"
    z={40}
    title={`needs you · ${items.length}`}
    onClose={() => (store.attentionOpen = false)}
  >
    <div class="list">
      {#if items.length === 0}
        <div class="empty">nothing is waiting — all clear</div>
      {/if}
      {#each items as item (item.session + item.reason)}
        <button class="row" onclick={() => jump(item.session)}>
          <div class="l1">
            <span class="name">{item.title}</span>
            <span class="ws">{wsName(item.workspace)}</span>
          </div>
          <div class="reason">{item.reason}</div>
          <div class="meta">{item.since > 0 ? fmtAgo(item.since, store.now) : 'now'}</div>
        </button>
      {/each}
    </div>
  </Overlay>
{/if}

<style>
  :global(.sheet) {
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 420px;
    max-height: 60vh;
    --head-color: var(--red);
  }
  .list {
    overflow: auto;
    min-height: 0;
  }
  .empty {
    padding: 16px 12px;
    font-size: 12px;
    color: var(--faint);
  }
  .row {
    display: block;
    width: 100%;
    background: none;
    border: none;
    border-bottom: 1px solid var(--line);
    padding: 8px 12px;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: rgba(240, 85, 63, 0.07);
  }
  .l1 {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .name {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--tx);
  }
  .ws {
    font: 10px var(--mono);
    color: var(--faint);
  }
  .reason {
    margin-top: 2px;
    font-size: 11.5px;
    color: var(--dim);
  }
  .meta {
    margin-top: 2px;
    font: 10px var(--mono);
    color: var(--faint);
  }
</style>
