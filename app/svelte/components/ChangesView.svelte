<script lang="ts">
  // The file-content view (file-tree plan): the file's actual content in the
  // center column, with git-diff indicators in the gutter (added lines green,
  // deletions as red ghosts) and syntax highlighting (shiki). It tracks the
  // file live — the store re-fetches its content + base on file_tree_changed.
  import { store, setChangesView } from '../lib/store.svelte';
  import { buildDiffLines, assemble, langFor, type RenderLine } from '../lib/diffview';
  import { highlightLines } from '../lib/highlight';

  const view = $derived(store.changesView);
  const next = $derived(store.changesViewNew);
  const base = $derived(store.changesViewBase);
  const git = $derived(view ? (store.gitStatus[view.workspace] ?? null) : null);

  // The switcher lists every changed file; the current file stays put when
  // the status refreshes around it.
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

  // Render lines (line number + diff mark + syntax spans), recomputed when
  // the content, base, or language changes. The async highlight is guarded so
  // a slow recompute never clobbers a newer one.
  let lines = $state<RenderLine[]>([]);
  let loading = $state(true);

  $effect(() => {
    const v = view;
    const n = next;
    const b = base;
    if (!v || n === null) {
      lines = [];
      loading = true;
      return;
    }
    let live = true;
    void (async () => {
      const tokens = await highlightLines(n, langFor(v.path));
      if (!live) return;
      lines = assemble(buildDiffLines(b ?? '', n), tokens);
      loading = false;
    })();
    return () => {
      live = false;
    };
  });

  function pick(p: string): void {
    // Switcher files are changed (git status), never .gitignored.
    if (view) setChangesView({ ...view, path: p, ignored: false });
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
      {#if loading}
        <div class="note">loading…</div>
      {:else if lines.length === 0}
        <div class="note">empty file</div>
      {:else}
        <pre class="code">
          {#each lines as rl (rl.key)}
            <div class="line" class:added={rl.mark === 'add'} class:removed={rl.mark === 'del'}>
              <span class="ln">{rl.no}</span>
              <span class="mk">{rl.mark === 'add' ? '+' : rl.mark === 'del' ? '−' : ''}</span>
              <span class="src">{#each rl.spans as s (s)}<span style:color={s.color}>{s.text}</span>{/each}</span>
            </div>
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
  /* The file body: shiki's dark-plus foreground by default; per-token
     spans override with the grammar's colors. */
  .code {
    margin: 0;
    padding: 8px 0 24px;
    font: 11.5px/1.4 var(--mono);
    color: #d4d4d4;
  }
  .line {
    display: flex;
  }
  .ln {
    flex: none;
    width: 4ch;
    text-align: right;
    padding: 0 8px 0 12px;
    color: var(--faint);
    user-select: none;
  }
  .mk {
    flex: none;
    width: 1.2ch;
    text-align: center;
    color: transparent;
    user-select: none;
  }
  .src {
    flex: 1;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding-right: 16px;
  }
  .line.added {
    background: rgba(63, 185, 80, 0.14);
  }
  .line.added .mk {
    color: #7ee08a;
  }
  .line.added .ln {
    color: rgba(126, 224, 138, 0.7);
  }
  .line.removed {
    background: rgba(240, 85, 63, 0.14);
  }
  .line.removed .mk {
    color: #ff9d8f;
  }
  .line.removed .src {
    color: #ff9d8f;
  }
  .line.removed .ln {
    color: transparent;
  }
</style>
