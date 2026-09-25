<script lang="ts">
  // One node of the files tree (ticket #32): a dir row toggles its listing
  // (the store fetches on first expand, FileTreeChanged keeps it fresh); a
  // file row is inert — v0 has no file preview. The row shell is the shared
  // TreeNode; the recursion is the self-import (svelte:self is deprecated
  // and svelte-check chokes on its renamed props).
  import FileNode from './FileNode.svelte';
  import TreeNode from './TreeNode.svelte';
  import { store, toggleFileDir, retryDirFetch } from '../lib/store.svelte';
  import type { FileEntry } from '../lib/protocol';

  let { entry, ws, depth = 0 }: { entry: FileEntry; ws: string; depth?: number } = $props();

  const children = $derived(store.files[ws]?.[entry.path] ?? null);

  function toggle(): void {
    if (!entry.dir) return;
    if (store.fileErrors[ws]?.[entry.path]) {
      // A failed dir is already listed (empty): toggleFileDir would just
      // collapse it, so the row re-fetches instead.
      retryDirFetch(ws, entry.path);
      return;
    }
    toggleFileDir(ws, entry.path);
  }
</script>

{#snippet rowLabel()}
  <span class="name">{entry.name}</span>
{/snippet}
<TreeNode
  {depth}
  expanded={entry.dir ? children !== null : null}
  onRow={toggle}
  onToggle={toggle}
  label={rowLabel}
/>
{#if entry.dir && children}
  <div class="kids">
    {#each children as c (c.path)}
      <FileNode entry={c} ws={ws} depth={depth + 1} />
    {/each}
    {#if store.fileErrors[ws]?.[entry.path]}
      <div class="fail">failed to list — click the row to retry</div>
    {/if}
  </div>
{/if}

<style>
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kids {
    display: flex;
    flex-direction: column;
  }
  .fail {
    padding: 2px 8px;
    font: 10.5px var(--mono);
    color: var(--red);
  }
</style>
