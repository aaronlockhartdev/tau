<script lang="ts">
  // The attention sheet (workbench P5): the bell's overlay. A short list of
  // everything parked on the user, across all open workspaces; a jump opens
  // the session (the workspace context follows the session).
  import { store, attention, switchSession } from '../lib/store.svelte';
  import { fmtAgo } from '../lib/format';

  const open = $derived(store.attentionOpen);
  const items = $derived(attention());
  const wsName = (id: string) => store.workspaces.find((w) => w.id === id)?.name ?? id;

  function jump(session: string): void {
    store.attentionOpen = false;
    void switchSession(session);
  }
</script>

{#if open}
  <div class="dim" role="presentation" onclick={() => (store.attentionOpen = false)}></div>
  <div class="sheet" role="dialog" aria-label="Needs your attention">
    <div class="head">
      <span>needs you · {items.length}</span>
      <button class="x" title="close" onclick={() => (store.attentionOpen = false)}>×</button>
    </div>
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
          <div class="meta">{item.since > 0 ? fmtAgo(item.since) : 'now'}</div>
        </button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .dim {
    position: fixed;
    inset: 0;
    background: rgba(5, 6, 8, 0.72);
    z-index: 40;
  }
  .sheet {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 420px;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: var(--raised);
    border: 1px solid var(--line2);
    border-radius: 10px;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.55);
    z-index: 41;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    font: 700 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--red);
    flex: none;
  }
  .x {
    background: none;
    border: none;
    color: var(--dim);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 5px;
  }
  .x:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--tx);
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
