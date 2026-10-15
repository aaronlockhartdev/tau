<script lang="ts">
  // The file viewer (open-files model): a persistent, per-workspace set of open
  // files as closable tabs, independent of git status. The active tab's content
  // renders with git-diff gutter marks and syntax highlighting (shiki), tracked
  // live — the store re-fetches it on file_tree_changed. The center column
  // toggles between this and the transcript (pane.center); the "chat" control
  // and Esc leave the viewer without closing any file.
  import { store, closeFile, selectFile, setCenterMode } from '../lib/store.svelte';
  import { buildDiffLines, assemble, langFor, type RenderLine } from '../lib/diffview';
  import { highlightLines } from '../lib/highlight';
  import { onActivate } from '../lib/ui';
  import { MessageSquare } from '@lucide/svelte';

  const active = $derived(store.activeFile);
  const ws = $derived(active?.workspace ?? null);
  const openList = $derived(active ? (store.openFiles[active.workspace] ?? []) : []);
  const next = $derived(store.activeFileNew);
  const base = $derived(store.activeFileBase);
  const err = $derived(store.activeFileError);

  // Render lines (line number + diff mark + syntax spans), recomputed when the
  // content, base, or language changes. The async highlight is guarded so a slow
  // recompute never clobbers a newer one; a highlight failure (a prod WASM/grammar
  // init rejection) falls back to plain, uncolored lines rather than sticking.
  let lines = $state<RenderLine[]>([]);
  $effect(() => {
    const a = active;
    const n = next;
    const b = base;
    if (!a || n === null) {
      lines = [];
      return;
    }
    let live = true;
    void (async () => {
      let tokens;
      try {
        tokens = await highlightLines(n, langFor(a.path));
      } catch {
        tokens = n.split('\n').map((l) => [{ content: l }]);
      }
      if (!live) return;
      lines = assemble(buildDiffLines(b ?? '', n), tokens);
    })();
    return () => {
      live = false;
    };
  });
</script>

{#if active && ws}
  <div class="view">
    <div class="bar">
      <button class="back" title="back to transcript" onclick={() => setCenterMode(ws, 'chat')}>
        <MessageSquare size={12} aria-hidden="true" />
        <span>chat</span>
      </button>
      <div class="tabs">
        {#each openList as f (f.path)}
          <div class="tab" class:on={f.path === active.path} title={f.path}>
            <span
              class="tname"
              role="button"
              tabindex="0"
              onclick={() => selectFile(ws, f.path)}
              onkeydown={onActivate(() => selectFile(ws, f.path))}
            >{f.path.split('/').pop()}</span>
            <span
              class="tx"
              title="close"
              role="button"
              tabindex="0"
              onclick={() => closeFile(ws, f.path)}
              onkeydown={onActivate(() => closeFile(ws, f.path))}
            >×</span>
          </div>
        {/each}
      </div>
    </div>
    <div class="body">
      {#if err !== null}
        <div class="note">{err}</div>
      {:else if next === null}
        <div class="note">loading…</div>
      {:else if next === ''}
        <div class="note">empty file</div>
      {:else}
        <div class="code">
          {#each lines as rl (rl.key)}
            <div class="line" class:added={rl.mark === 'add'} class:removed={rl.mark === 'del'}>
              <span class="ln">{rl.no}</span>
              <span class="mk">{rl.mark === 'add' ? '+' : rl.mark === 'del' ? '−' : ''}</span>
              <span class="src">{#each rl.spans as s (s)}<span style:color={s.color}>{s.text}</span>{/each}</span>
            </div>
          {/each}
        </div>
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
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .back {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    padding: 3px 8px;
    border: 1px solid var(--line2);
    border-radius: 5px;
    color: var(--dim);
    font: 10.5px var(--mono);
  }
  .back:hover {
    color: var(--tx);
    border-color: var(--acc);
  }
  .back :global(svg) {
    flex: none;
  }
  .tabs {
    display: flex;
    gap: 4px;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
  }
  .tab {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    padding: 2px 4px 2px 8px;
    border: 1px solid var(--line2);
    border-radius: 5px;
    color: var(--dim);
    font: 10.5px var(--mono);
  }
  .tab.on {
    color: var(--acc);
    border-color: var(--acc);
    background: rgba(76, 141, 255, 0.1);
  }
  .tname {
    cursor: pointer;
    white-space: nowrap;
  }
  .tx {
    cursor: pointer;
    padding: 0 3px;
    border-radius: 3px;
    color: var(--faint);
    font-size: 13px;
    line-height: 1;
  }
  .tx:hover {
    color: var(--tx);
    background: rgba(255, 255, 255, 0.08);
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
