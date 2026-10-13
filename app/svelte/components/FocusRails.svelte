<script lang="ts">
  // Focus-mode pane menus (workbench P6, second restyle): small floating
  // segmented chips in the top corners — the same control language as the
  // pane tab headers — replacing the full-height edge rails. A click
  // points the pane at a tab, then toggles a non-modal inspector sheet
  // (the real side pane) beside it.
  import { store, pane, currentSession } from '../lib/store.svelte';
  import LeftPane from './LeftPane.svelte';
  import RightPane from './RightPane.svelte';

  const on = $derived(store.focus);
  const side = $derived(store.focusInspector);
  const ws = $derived(currentSession()?.meta.workspace ?? null);
  const q = $derived(ws ? pane(ws) : null);

  function chip(s: 'left' | 'right', tab: string): void {
    if (q) {
      if (s === 'left') q.ltab = tab as 'files' | 'sessions';
      else q.rtab = tab as 'tasks' | 'subs';
    }
    store.focusInspector = store.focusInspector === s ? null : s;
  }
</script>

{#if on}
  <div class="chips left" class:shift={side === 'left'}>
    <button
      class="chip"
      class:on={side === 'left' && q?.ltab === 'sessions'}
      title="sessions"
      onclick={() => chip('left', 'sessions')}
    >
      <svg><use href="#i-book" /></svg>
    </button>
    <button
      class="chip"
      class:on={side === 'left' && q?.ltab === 'files'}
      title="files"
      onclick={() => chip('left', 'files')}
    >
      <svg><use href="#i-file" /></svg>
    </button>
  </div>
  <div class="chips right" class:shift={side === 'right'}>
    <button
      class="chip"
      class:on={side === 'right' && q?.rtab === 'tasks'}
      title="tasks"
      onclick={() => chip('right', 'tasks')}
    >
      <svg><use href="#i-check" /></svg>
    </button>
    <button
      class="chip"
      class:on={side === 'right' && q?.rtab === 'subs'}
      title="sub-agents"
      onclick={() => chip('right', 'subagents')}
    >
      <svg><use href="#i-bot" /></svg>
    </button>
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
{/if}

<style>
  .chips {
    position: absolute;
    top: 10px;
    display: flex;
    gap: 2px;
    padding: 2px;
    background: var(--panel2);
    border: 1px solid var(--line);
    border-radius: 8px;
    z-index: 31;
  }
  .chips.left {
    left: 12px;
  }
  /* When the sheet is open the chips step out past it, staying clickable. */
  .chips.left.shift {
    left: 316px;
  }
  .chips.right {
    right: 12px;
  }
  .chips.right.shift {
    right: 316px;
  }
  .chip {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 24px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--dim);
    cursor: pointer;
  }
  .chip svg {
    width: 14px;
    height: 14px;
    stroke: currentColor;
    fill: none;
    stroke-width: 1.8;
  }
  .chip:hover {
    color: var(--tx);
  }
  .chip.on {
    color: var(--tx);
    background: var(--raised);
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
    z-index: 30;
  }
  .insp.insp-left {
    left: 12px;
  }
  .insp:not(.insp-left) {
    right: 12px;
  }
</style>
