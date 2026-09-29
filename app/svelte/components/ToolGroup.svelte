<script lang="ts">
  // A batch of parallel tool calls: a small indicator (the count) over the
  // individual cards, which stay visible (not collapsible). Parallel calls in
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
</script>

<div class="tool-group">
  <div class="tg-badge">⊞ {entry.children.length} batched</div>
  {#each entry.children as child (child.id)}
    <EntryCard entry={child as CardEntry} heightKey={`${heightKey}:${child.id}`} {turn} />
  {/each}
</div>

<style>
  .tool-group {
    margin: 0 16px 4px;
  }
  .tg-badge {
    font: 11px var(--mono);
    color: var(--muted);
    padding: 2px 8px;
    margin-bottom: 2px;
  }
</style>
