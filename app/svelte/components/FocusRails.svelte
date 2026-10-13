<script lang="ts">
  // Focus-mode pane handles (workbench P6, third restyle): one vertical
  // pill per window edge, mid-height — a drawer handle. It toggles that
  // side's inspector sheet (the real side pane); the tab choice lives in
  // the pane's own labelled header inside the sheet, and the pane's tab
  // state persists across toggles. The corner chips were too small to
  // aim at and too far from the working area.
  import { store } from '../lib/store.svelte';
  import { ChevronDown } from '@lucide/svelte';
  import LeftPane from './LeftPane.svelte';
  import RightPane from './RightPane.svelte';

  const on = $derived(store.focus);
  const side = $derived(store.focusInspector);

  function toggle(s: 'left' | 'right'): void {
    store.focusInspector = store.focusInspector === s ? null : s;
  }
</script>

{#if on}
  <button
    class="handle left"
    class:on={side === 'left'}
    title="show files & sessions"
    onclick={() => toggle('left')}
  >
    <ChevronDown size={14} />
  </button>
  <button
    class="handle right"
    class:on={side === 'right'}
    title="show tasks & sub-agents"
    onclick={() => toggle('right')}
  >
    <ChevronDown size={14} />
  </button>
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
  .handle {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 22px;
    height: 64px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--panel2);
    border: 1px solid var(--line2);
    color: var(--dim);
    cursor: pointer;
    z-index: 31;
  }
  .handle.left {
    left: 0;
    border-left: none;
    border-radius: 0 8px 8px 0;
  }
  .handle.right {
    right: 0;
    border-right: none;
    border-radius: 8px 0 0 8px;
  }
  .handle:hover {
    color: var(--tx);
    border-color: var(--acc);
  }
  .handle :global(svg) {
    display: block;
  }
  /* The chevron points at the pane it summons; open state points back. */
  .handle.left :global(svg) {
    transform: rotate(90deg);
  }
  .handle.left.on :global(svg) {
    transform: rotate(-90deg);
  }
  .handle.right :global(svg) {
    transform: rotate(-90deg);
  }
  .handle.right.on :global(svg) {
    transform: rotate(90deg);
  }
  .handle.on {
    color: var(--acc);
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
    z-index: 30;
  }
  .insp.insp-left {
    left: 0;
    border-left: none;
  }
  .insp:not(.insp-left) {
    right: 0;
    border-right: none;
  }
</style>
