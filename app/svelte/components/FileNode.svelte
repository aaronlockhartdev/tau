<script lang="ts">
  // One node of the files tree (ticket #32): a dir row toggles its listing
  // (the store fetches on first expand, FileTreeChanged keeps it fresh); a
  // file row is inert — v0 has no file preview. The row shell is the shared
  // TreeNode; the recursion is the self-import (svelte:self is deprecated
  // and svelte-check chokes on its renamed props).
  import FileNode from './FileNode.svelte';
  import TreeNode from './TreeNode.svelte';
  import { store, toggleFileDir, retryDirFetch, fileCache, setPeek } from '../lib/store.svelte';
  import type { FileEntry } from '../lib/protocol';

  // A file's git badge (P4): D beats A beats M when a path appears in more
  // than one status list.
  const gitBadge = $derived.by((): 'M' | 'A' | 'D' | null => {
    if (entry.dir) return null;
    const s = store.gitStatus[ws];
    if (!s) return null;
    if (s.deleted.includes(entry.path)) return 'D';
    if (s.untracked.includes(entry.path)) return 'A';
    if (s.modified.includes(entry.path)) return 'M';
    return null;
  });

  let { entry, ws, depth = 0 }: { entry: FileEntry; ws: string; depth?: number } = $props();

  const children = $derived(fileCache(ws)?.[entry.path] ?? null);

  function toggle(): void {
    if (!entry.dir) {
      // A file row opens the peek sheet (P4); dirs toggle their listing.
      setPeek({ workspace: ws, path: entry.path });
      return;
    }
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
  {#if gitBadge}
    <span class="gitb {gitBadge}">{gitBadge}</span>
  {/if}
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
  .gitb {
    flex: none;
    width: 12px;
    text-align: center;
    font: 700 9.5px var(--mono);
  }
  .gitb.M {
    color: var(--amber);
  }
  .gitb.A {
    color: var(--green);
  }
  .gitb.D {
    color: var(--red);
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
