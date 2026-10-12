<script lang="ts">
  // The peek sheet (workbench P4): a non-modal 470px diff viewer anchored to
  // the right edge of the center column. The center column is always the
  // transcript — the sheet overlays it, it never replaces it.
  import { store, setPeek } from '../lib/store.svelte';

  const peek = $derived(store.peek);
  const diff = $derived(store.peekDiff);
  const git = $derived(peek ? (store.gitStatus[peek.workspace] ?? null) : null);

  // The switcher lists every changed file in the workspace; the current
  // peek stays put when the status refreshes around it.
  const files = $derived.by((): string[] => {
    if (!peek || !git) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const p of [...git.modified, ...git.untracked, ...git.deleted]) {
      if (!seen.has(p)) {
        seen.add(p);
        out.push(p);
      }
    }
    if (peek && !seen.has(peek.path)) out.push(peek.path);
    return out;
  });

  const lines = $derived.by(() => (diff ?? '').split('\n'));
  const hasDiff = $derived(
    peek ? !!(git && (git.modified.includes(peek.path) || git.deleted.includes(peek.path))) : false
  );

  function pick(p: string): void {
    if (peek) setPeek({ ...peek, path: p });
  }
</script>

{#if peek}
  <div class="peek">
    <div class="ph">
      <div class="title" title={peek.path}>{peek.path}</div>
      <button class="x" title="close (esc)" onclick={() => setPeek(null)}>×</button>
    </div>
    {#if files.length > 1}
      <div class="switcher">
        {#each files as f (f)}
          <button class="f" class:on={f === peek.path} onclick={() => pick(f)} title={f}>
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
  .peek {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 470px;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border-left: 1px solid var(--line2);
    box-shadow: -12px 0 32px rgba(0, 0, 0, 0.45);
    z-index: 20;
  }
  .ph {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
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
    padding: 6px 10px;
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
    padding: 16px 12px;
    font-size: 12px;
    color: var(--faint);
  }
  .diff {
    margin: 0;
    padding: 8px 0;
    font: 11px/1.6 var(--mono);
  }
  .diff > div {
    padding: 0 12px;
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
