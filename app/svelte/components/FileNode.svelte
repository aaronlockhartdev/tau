<script lang="ts">
  // One row of the files tree (ticket #32): a dir row toggles its listing
  // (the store fetches on first expand, FileTreeChanged keeps it fresh);
  // a file row is inert — v0 has no file preview. The recursive pattern is
  // the self-import (svelte:self is deprecated and svelte-check chokes on
  // its renamed props).
  import FileNode from './FileNode.svelte';
  import { store, toggleFileDir } from '../lib/store.svelte';
  import type { FileEntry } from '../lib/protocol';

  let { entry, ws, depth = 0 }: { entry: FileEntry; ws: string; depth?: number } = $props();

  const children = $derived(store.files[ws]?.[entry.path] ?? null);

  function toggle(): void {
    if (entry.dir) toggleFileDir(ws, entry.path);
  }

</script>

<div class="frow" style="padding-left: {8 + depth * 14}px" role="button" tabindex={0}
     aria-expanded={entry.dir ? children !== null : undefined} onclick={toggle}
     onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggle(); } }}>
  {#if entry.dir}
    <span class="chev">{children ? '▾' : '▸'}</span>
  {:else}
    <span class="chev"></span>
  {/if}
  <span class="name">{entry.name}</span>
</div>
{#if entry.dir && children}
  <div class="kids">
    {#each children as c (c.path)}
      <FileNode entry={c} ws={ws} depth={depth + 1} />
    {/each}
  </div>
{/if}

<style>
  .frow {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4.5px 12px;
    cursor: pointer;
    font-size: 12.5px;
  }
  .frow .chev {
    width: 10px;
    flex: none;
    color: var(--faint);
    font-size: 10px;
  }
  .frow .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kids {
    display: flex;
    flex-direction: column;
  }
</style>
