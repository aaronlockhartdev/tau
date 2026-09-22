<script lang="ts">
  // The row shell shared by the left pane's trees (sessions, files): the
  // depth indent, the rotating chevron, and the row's a11y. The row's
  // content and its behavior (open, toggle, rename) come from the caller;
  // the recursion stays with each tree.
  import type { Snippet } from 'svelte';

  let {
    depth = 0,
    expanded = null,
    selected = false,
    label,
    onRow,
    onRowDbl,
    onToggle
  }: {
    depth?: number;
    // null = no children (no chevron); true/false = group open/closed.
    expanded?: boolean | null;
    selected?: boolean;
    label: Snippet;
    onRow?: () => void;
    onRowDbl?: () => void;
    onToggle?: () => void;
  } = $props();

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      if (onRow) onRow();
      else onToggle?.();
    }
  }
</script>

<div class="node" style:--indent="{depth * 10}px">
  <div
    class="trow"
    class:sel={selected}
    role="button"
    tabindex={0}
    aria-expanded={expanded === null ? undefined : expanded}
    onclick={onRow}
    ondblclick={onRowDbl}
    onkeydown={onKey}
  >
    <button
      class="chev"
      class:has={expanded !== null}
      class:open={expanded === true}
      type="button"
      disabled={expanded === null}
      onclick={(e) => {
        e.stopPropagation();
        onToggle?.();
      }}
    >
      {#if expanded !== null}
        <svg class="ci" width="11" height="11"><use href="#i-chev"/></svg>
      {/if}
    </button>
    {@render label()}
  </div>
</div>

<style>
  .trow {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 4.5px 8px 4.5px calc(var(--indent, 0px) + 8px);
    cursor: pointer;
    font-size: 12.5px;
    line-height: 1;
  }
  .trow:hover {
    background: var(--panel2);
  }
  .trow.sel {
    background: color-mix(in srgb, var(--acc) 8%, transparent);
  }
  .trow .chev {
    width: 10px;
    flex: none;
    color: var(--dim);
  }
  .trow .chev .ci {
    display: block;
    transition: transform 0.12s;
  }
  .trow .chev:not(.open) .ci {
    transform: rotate(-90deg);
  }
  .trow .chev:not(.has) {
    visibility: hidden;
  }
</style>
