<script lang="ts">
  // Two-section vertical queue above the composer (spec §9):
  // steering ("next opportunity") on top, follow-up ("after work
  // completes") below. Items are deletable; the core's queue events are
  // the source of truth, so deletion is optimistic with resync.

  import { store, deleteQueueItem } from '../lib/store.svelte';

  const cur = $derived(store.current);
  const s = $derived(cur ? store.sessions[cur] : null);
</script>

{#if s && s.pending.length > 0}
  <div class="queue">
    {#if s.pending.some((p) => p.lane === 'steering')}
      <div class="qlabel">next opportunity</div>
    {/if}
    {#each s.pending.filter((p) => p.lane === 'steering') as p (p.text)}
      <div class="qrow">
        <span class="qtext">{p.text}</span>
        <button class="qdel" onclick={() => deleteQueueItem(p.text, p.lane)}>✕</button>
      </div>
    {/each}
    {#if s.pending.some((p) => p.lane === 'follow-up')}
      <div class="qlabel">after work completes</div>
    {/if}
    {#each s.pending.filter((p) => p.lane === 'follow-up') as p (p.text)}
      <div class="qrow">
        <span class="qtext">{p.text}</span>
        <button class="qdel" onclick={() => deleteQueueItem(p.text, p.lane)}>✕</button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .queue {
    flex: none;
    padding: 8px 16px;
    border-top: 1px solid var(--line);
    background: var(--panel);
  }
  .qlabel {
    font: 10px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--dim);
    margin: 4px 0;
  }
  .qrow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 8px;
    font-size: 12.5px;
    border-radius: 4px;
  }
  .qrow:hover {
    background: var(--panel2);
  }
  .qtext {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .qdel {
    color: var(--dim);
    font-size: 11px;
    padding: 0 3px;
  }
  .qdel:hover {
    color: var(--red);
  }
</style>
