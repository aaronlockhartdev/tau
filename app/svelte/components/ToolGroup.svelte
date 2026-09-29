<script lang="ts">
  // A batch of parallel tool calls, rendered as one collapsible group: a
  // shared header (the count) over the individual cards. Parallel calls in
  // one LLM response land back-to-back in the file, so a run is a batch.
  import type { CardEntry, Entry } from '../lib/protocol';
  import EntryCard from './EntryCard.svelte';

  type ToolGroupEntry = Extract<Entry, { kind: 'tool_group' }>;

  let {
    entry,
    heightKey,
    turn = ''
  }: {
    entry: ToolGroupEntry;
    heightKey: string;
    turn?: 'you' | 'agent' | '';
  } = $props();

  let open = $state(false);
</script>

<div class="tool-group">
  <div
    class="tg-hd"
    role="button"
    tabindex="0"
    onclick={() => (open = !open)}
    onkeydown={(ev) => {
      if (ev.key === 'Enter' || ev.key === ' ') open = !open;
    }}
  >
    <span class="tg-count">{entry.children.length}</span>
    <span class="tg-label">tool calls</span>
    <span class="tg-chev">{open ? '▾' : '▸'}</span>
  </div>
  {#if open}
    <div class="tg-body">
      {#each entry.children as child (child.id)}
        <EntryCard entry={child as CardEntry} heightKey={`${heightKey}:${child.id}`} {turn} />
      {/each}
    </div>
  {/if}
</div>

<style>
  .tool-group {
    margin: 0 16px 4px;
  }
  .tg-hd {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    font-size: 12px;
    color: var(--muted);
    cursor: pointer;
    user-select: none;
    border-radius: 6px;
  }
  .tg-hd:hover {
    background: var(--panel2);
  }
  .tg-count {
    font-weight: 600;
    color: var(--text);
  }
  .tg-chev {
    margin-left: auto;
    font-size: 10px;
  }
  .tg-body {
    margin-top: 2px;
  }
</style>
