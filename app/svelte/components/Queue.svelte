<script lang="ts">
  // Two-section vertical queue above the composer (spec §9):
  // steering ("next tool boundary") on top, follow-up ("after work
  // completes") below. Child reports (a sub-agent's wake) carry a source
  // and get their own section, distinct from the user's own messages.
  // Items are deletable; the core's queue events are the source of truth,
  // so deletion is optimistic with resync.

  import { deleteQueueItem, currentSession } from '../lib/store/store.svelte';
  import type { PendingMsg } from '../lib/rules/sessions';

  const s = $derived(currentSession());
  const reports = $derived(s ? s.pending.filter((p) => p.source && p.source !== 'user') : []);
  const steering = $derived(s ? s.pending.filter((p) => p.lane === 'steering' && (!p.source || p.source === 'user')) : []);
  const followUp = $derived(s ? s.pending.filter((p) => p.lane === 'follow-up' && (!p.source || p.source === 'user')) : []);
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
    {@render queueSection('next tool boundary', steering, false)}
    {@render queueSection('after work completes', followUp, false)}
  </div>
{/if}

<style>
  .queue {
    flex: none;
    margin: 0 8px 10px;
    padding: 4px 8px;
  }
  .qlabel {
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--faint);
    margin: 8px 0 2px;
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
    min-width: 0;
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
