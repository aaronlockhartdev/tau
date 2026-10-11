<script lang="ts">
  // Two-section vertical queue above the composer (spec §9):
  // steering ("next opportunity") on top, follow-up ("after work
  // completes") below. Child reports (a sub-agent's wake) carry a source
  // and get their own section, distinct from the user's own messages.
  // Items are deletable; the core's queue events are the source of truth,
  // so deletion is optimistic with resync.

  import { deleteQueueItem, currentSession } from '../lib/store.svelte';
  import type { PendingMsg } from '../lib/sessions';

  const s = $derived(currentSession());
  const reports = $derived(s ? s.pending.filter((p) => p.source) : []);
  const steering = $derived(s ? s.pending.filter((p) => p.lane === 'steering' && !p.source) : []);
  const followUp = $derived(s ? s.pending.filter((p) => p.lane === 'follow-up' && !p.source) : []);
</script>

{#if s && s.pending.length > 0}
  <div class="queue">
    {#snippet queueSection(heading: string, items: PendingMsg[], sub: boolean)}
      {#if items.length > 0}
        <div class="qlabel">{heading}</div>
      {/if}
      {#each items as p, idx (p.text + ':' + idx)}
        <div class="qrow" class:qsub={sub}>
          {#if sub}
            <span class="qsrc" title={p.source ?? ''}>sub-agent {(p.source ?? '').split('-').pop()}</span>
          {/if}
          <span class="qtext">{p.text}</span>
          {#if !sub}
            <button class="qdel" onclick={() => void deleteQueueItem(p.text, p.lane, null, idx)}>✕</button>
          {/if}
        </div>
      {/each}
    {/snippet}
    {@render queueSection('subagent reports', reports, true)}
    {@render queueSection('next opportunity', steering, false)}
    {@render queueSection('after work completes', followUp, false)}
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
