<script lang="ts">
  // One node of the files tree (ticket #32): a dir row toggles its listing
  // (the store fetches on first expand, FileTreeChanged keeps it fresh); a
  // file row is inert — v0 has no file preview. The row shell is the shared
  // TreeNode; the recursion is the self-import (svelte:self is deprecated
  // and svelte-check chokes on its renamed props).
  import FileNode from './FileNode.svelte';
  import TreeNode from './TreeNode.svelte';
  import { store, toggleFileDir, retryDirFetch, fileCache, setPeek } from '../lib/store.svelte';
  import { fileIcon, fileKind } from '../lib/files';
  import type { FileEntry } from '../lib/protocol';


  let { entry, ws, depth = 0 }: { entry: FileEntry; ws: string; depth?: number } = $props();

  const children = $derived(fileCache(ws)?.[entry.path] ?? null);

  // Git awareness (file-tree plan, phase 2): the entry's change state
  // tints the name; numstat counts (when measurable) render as +N −M.
  // The P4 letter badge retires — tint + stats replace it.
  const gitState = $derived(entry.git?.state ?? null);
  const stats = $derived.by(() => {
    const g = entry.git;
    if (!g || g.added == null || g.deleted == null) return null;
    return { added: g.added, deleted: g.deleted };
  });

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
  {@const Icon = fileIcon(entry.name, entry.dir)}
  <Icon class="fic k-{fileKind(entry.name, entry.dir)}" size={14} />
  <span class="name {gitState ? `tint ${gitState}` : ''}">{entry.name}</span>
  {#if stats}
    <span class="stats"><span class="add">+{stats.added}</span> <span class="del">−{stats.deleted}</span></span>
  {/if}
{/snippet}
<div class:ign={entry.ignored}>
  <TreeNode
    {depth}
    expanded={entry.dir ? children !== null : null}
    onRow={toggle}
    onToggle={toggle}
    label={rowLabel}
  />
</div>
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
  :global(.fic) {
    flex: none;
    opacity: 0.85;
  }
  :global(.k-code) {
    color: var(--acc);
  }
  :global(.k-json),
  :global(.k-git) {
    color: var(--amber);
  }
  :global(.k-doc) {
    color: var(--green);
  }
  :global(.k-conf) {
    color: var(--purple);
  }
  :global(.k-img),
  :global(.k-dir) {
    color: var(--dim);
  }
  :global(.k-gen) {
    color: var(--faint);
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  :global(.tint.modified) {
    color: var(--amber);
  }
  :global(.tint.untracked) {
    color: var(--green);
  }
  :global(.tint.deleted) {
    color: var(--red);
  }
  .stats {
    flex: none;
    font: 10.5px var(--mono);
  }
  .stats .add {
    color: var(--green);
  }
  .stats .del {
    color: var(--red);
  }
  .ign {
    opacity: 0.45;
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
