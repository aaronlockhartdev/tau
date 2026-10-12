<script lang="ts">
  // The focus-mode rails (workbench P6): edge pill strips that open
  // non-modal inspector sheets — the real side panes, floating beside the
  // rail — instead of nothing. A pill points its pane at a tab, then
  // toggles the sheet.
  import { store, pane, currentSession } from '../lib/store.svelte';
  import LeftPane from './LeftPane.svelte';
  import RightPane from './RightPane.svelte';

  const on = $derived(store.focus);
  const side = $derived(store.focusInspector);

  function railPill(s: 'left' | 'right', tab: string): void {
    const ws = currentSession()?.meta.workspace ?? null;
    const q = ws ? pane(ws) : null;
    if (q) {
      if (s === 'left') q.ltab = tab as 'files' | 'sessions';
      else q.rtab = tab as 'tasks' | 'subs';
    }
    store.focusInspector = store.focusInspector === s ? null : s;
  }
</script>

{#if on}
  <div class="rails">
    <div class="rail rleft">
      <button class="pill" class:on={side === 'left'} onclick={() => railPill('left', 'sessions')}>sessions</button>
      <button class="pill" class:on={side === 'left'} onclick={() => railPill('left', 'files')}>files</button>
    </div>
    <div class="rail rright">
      <button class="pill" class:on={side === 'right'} onclick={() => railPill('right', 'tasks')}>tasks</button>
      <button class="pill" class:on={side === 'right'} onclick={() => railPill('right', 'subagents')}>sub-agents</button>
    </div>
    {#if side}
      <div class="insp" class:insp-left={side === 'left'}>
        {#if side === 'left'}
          <LeftPane />
        {:else}
          <RightPane />
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .rails {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 30;
  }
  .rail {
    position: absolute;
    top: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    pointer-events: auto;
  }
  .rail.rleft {
    left: 8px;
  }
  .rail.rright {
    right: 8px;
  }
  .pill {
    writing-mode: vertical-rl;
    padding: 8px 5px;
    border-radius: 10px;
    border: 1px solid var(--line2);
    background: var(--panel2);
    color: var(--dim);
    font: 10px var(--mono);
    letter-spacing: 0.04em;
    cursor: pointer;
  }
  .pill:hover {
    color: var(--tx);
  }
  .pill.on {
    background: color-mix(in srgb, var(--acc) 16%, var(--panel2));
    color: var(--tx);
    border-color: var(--acc);
  }
  .insp {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 300px;
    background: var(--panel);
    border: 1px solid var(--line2);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
    overflow: auto;
    pointer-events: auto;
  }
  .insp.insp-left {
    left: 40px;
  }
  .insp:not(.insp-left) {
    right: 40px;
  }
</style>
