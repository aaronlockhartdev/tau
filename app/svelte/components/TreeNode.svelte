<script lang="ts">
  // The row shell shared by the left pane's trees (sessions, files): the
  // depth indent, the rotating chevron, and the row's a11y. The row's
  // content and its behavior (open, toggle, rename) come from the caller;
  // the recursion stays with each tree.
  import type { Snippet } from 'svelte';
  import { isActivateKey } from '../lib/ui';
  import { ChevronDown } from '@lucide/svelte';

  let {
    depth = 0,
    expanded = null,
    selected = false,
    multi = false,
    dimmed = false,
    att = false,
    label,
    onRow,
    onRowDbl,
    onToggle,
    onContext
  }: {
    depth?: number;
    // null = no children (no chevron); true/false = group open/closed.
    expanded?: boolean | null;
    selected?: boolean;
    // A multiselect member (cmd/ctrl, shift range): same tint as the
    // active row.
    multi?: boolean;
    // A non-interactive row (an archived session): no rename/toggle,
    // dimmed — the row still takes selection clicks and the context
    // menu.
    dimmed?: boolean;
    // A needs-you row (P5): red left edge so the tree points at it.
    att?: boolean;
    label: Snippet;
    onRow?: (e: MouseEvent) => void;
    onRowDbl?: () => void;
    onToggle?: () => void;
    onContext?: (e: MouseEvent) => void;
  } = $props();

  function onKey(e: KeyboardEvent): void {
    if (isActivateKey(e.key)) {
      e.preventDefault();
      // The modifiers ride the keyboard event too; the row handler
      // only reads them.
      if (onRow) onRow(e as unknown as MouseEvent);
      else onToggle?.();
    }
  }
</script>

<div class="node" style:--indent="{depth * 10}px">
  <div
    class="trow"
    class:sel={selected}
    class:multi={multi}
    class:dimmed={dimmed}
    class:att={att}
    role="button"
    tabindex={0}
    aria-expanded={expanded === null ? undefined : expanded}
    onclick={onRow}
    ondblclick={dimmed ? undefined : onRowDbl}
    oncontextmenu={onContext}
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
        <ChevronDown class="ci" width="11" height="11" />
      {/if}
    </button>
    {@render label()}
  </div>
</div>

<style>
  .trow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px 6px calc(var(--indent, 0px) + 8px);
    border-radius: 8px;
    border: 1px solid transparent;
    cursor: pointer;
    font-size: 12.5px;
    line-height: 1;
  }
  .trow:hover {
    background: rgba(255, 255, 255, 0.04);
  }
  .trow.att {
    border-left: 2px solid var(--red);
    background: rgba(240, 85, 63, 0.06);
  }
  .trow.sel,
  .trow.multi {
    background: var(--raised);
    border-color: var(--line2);
  }
  .trow.dimmed {
    cursor: default;
    opacity: 0.55;
  }
  .trow.dimmed:hover {
    background: none;
  }
  .trow .chev {
    width: 10px;
    flex: none;
    color: var(--dim);
  }
  .trow .chev :global(.ci) {
    display: block;
    transition: transform 0.12s;
  }
  .trow .chev:not(.open) :global(.ci) {
    transform: rotate(-90deg);
  }
  .trow .chev:not(.has) {
    visibility: hidden;
  }
</style>
