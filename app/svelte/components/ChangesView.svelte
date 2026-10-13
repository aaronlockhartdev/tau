<script lang="ts">
  // The changes view (file-tree plan): the file's git diff in the center
  // column, replacing the transcript while open. A file row in the tree
  // opens it; the future editor lands in this same slot.
  import { store, setChangesView } from '../lib/store.svelte';

  const view = $derived(store.changesView);
  const diff = $derived(store.changesViewDiff);
  const git = $derived(view ? (store.gitStatus[view.workspace] ?? null) : null);

  // The switcher lists every changed file in the workspace; the current
  // file stays put when the status refreshes around it.
  const files = $derived.by((): string[] => {
    if (!view || !git) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const p of [...git.modified, ...git.untracked, ...git.deleted]) {
      if (!seen.has(p)) {
        seen.add(p);
        out.push(p);
      }
    }
    if (view && !seen.has(view.path)) out.push(view.path);
    return out;
  });

  const lines = $derived.by(() => (diff ?? '').split('\n'));
  const hasDiff = $derived(
    view ? !!(git && (git.modified.includes(view.path) || git.deleted.includes(view.path))) : false
  );

  function pick(p: string): void {
    if (view) setChangesView({ ...view, path: p });
  }
</script>

{#if view}
  <div class="view">
    <div class="hd">
      <div class="title" title={view.path}>{view.path}</div>
      <button class="x" title="close (esc)" onclick={() => setChangesView(null)}>×</button>
    </div>
    {#if files.length > 1}
      <div class="switcher">
        {#each files as f (f)}
          <button class="f" class:on={f === view.path} onclick={() => pick(f)} title={f}>
            {f.split('/').pop()}
          </button>
        {/each}
      </div>
    {/if}
    <div class="body">
      {#if diff === null}
        <div class="note">loading diff…</div>
      {:else if diff === ''}
        <div class="note">{hasDiff ? 'no changes' : 'no diff — untracked or not in git'}</div>
      {:else}
        <pre class="diff">
          {#each lines as l, i (i)}
            <div
              class:al={l.startsWith('+') && !l.startsWith('+++')}
              class:dl={l.startsWith('-') && !l.startsWith('---')}
              class:hh={l.startsWith('@@')}
            >{l}</div>
          {/each}
        </pre>
      {/if}
    </div>
  </div>
{/if}

<style>
  /* Same slot as the transcript: it fills the center column's reading
     column (and its focus-mode width) with the column's own background. */
  .view {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .hd {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .title {
    flex: 1;
    min-width: 0;
    font: 12px var(--mono);
    color: var(--tx);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .x {
    flex: none;
    background: none;
    border: none;
    color: var(--dim);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 5px;
  }
  .x:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--tx);
  }
  .switcher {
    display: flex;
    gap: 4px;
    padding: 6px 16px;
    border-bottom: 1px solid var(--line);
    overflow-x: auto;
    flex: none;
  }
  .f {
    flex: none;
    background: none;
    border: 1px solid var(--line2);
    border-radius: 5px;
    padding: 2px 8px;
    font: 10.5px var(--mono);
    color: var(--dim);
    cursor: pointer;
  }
  .f:hover {
    color: var(--tx);
    border-color: var(--acc);
  }
  .f.on {
    color: var(--acc);
    border-color: var(--acc);
    background: rgba(76, 141, 255, 0.1);
  }
  .body {
    flex: 1;
    overflow: auto;
    min-height: 0;
  }
  .note {
    padding: 16px;
    font-size: 12px;
    color: var(--faint);
  }
  .diff {
    margin: 0;
    padding: 8px 16px;
    font: 11px/1.6 var(--mono);
  }
  .diff > div {
    white-space: pre-wrap;
    word-break: break-all;
  }
  .diff .al {
    background: rgba(63, 185, 80, 0.13);
    color: #7ee08a;
  }
  .diff .dl {
    background: rgba(240, 85, 63, 0.13);
    color: #ff9d8f;
  }
  .diff .hh {
    color: var(--acc);
  }
</style>
