<script lang="ts">
  // Two-section vertical queue above the composer (spec §9):
  // steering ("next opportunity") on top, follow-up ("after work
  // completes") below. Child reports (a sub-agent's wake) carry a source
  // and get their own section, distinct from the user's own messages.
  // Items are deletable; the core's queue events are the source of truth,
  // so deletion is optimistic with resync.

  import { deleteQueueItem, currentSession } from '../lib/store.svelte';

  const s = $derived(currentSession());
  const reports = $derived(s ? s.pending.filter((p) => p.source) : []);
  const steering = $derived(s ? s.pending.filter((p) => p.lane === 'steering' && !p.source) : []);
  const followUp = $derived(s ? s.pending.filter((p) => p.lane === 'follow-up' && !p.source) : []);
</script>

{#if s && s.pending.length > 0}
  <div class="queue">
    {#if reports.length > 0}
      <div class="qlabel">subagent reports</div>
    {/if}
    {#each reports as p (p.text)}
      <div class="qrow qsub">
        <span class="qsrc" title={p.source ?? ''}>sub-agent {(p.source ?? '').split('-').pop()}</span>
        <span class="qtext">{p.text}</span>
      </div>
    {/each}
    {#if steering.length > 0}
      <div class="qlabel">next opportunity</div>
    {/if}
    {#each steering as p, idx (p.text + ':' + idx)}
      <div class="qrow">
        <span class="qtext">{p.text}</span>
        <button class="qdel" onclick={() => void deleteQueueItem(p.text, p.lane, null, idx)}>✕</button>
      </div>
    {/each}
    {#if followUp.length > 0}
      <div class="qlabel">after work completes</div>
    {/if}
    {#each followUp as p, idx (p.text + ':' + idx)}
      <div class="qrow">
        <span class="qtext">{p.text}</span>
        <button class="qdel" onclick={() => void deleteQueueItem(p.text, p.lane, null, idx)}>✕</button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .queue {
    flex: none;
    margin: 0 16px 10px;
    padding: 8px 12px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--panel);
  }
  .qlabel {
    font: 10.5px var(--mono);
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
  /* A child's report: set off from the user's own messages. */
  .qrow.qsub {
    border-left: 2px solid var(--amber);
  }
  .qsrc {
    flex: none;
    font: 10px var(--mono);
    color: var(--amber);
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
