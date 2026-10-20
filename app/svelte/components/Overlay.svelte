<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    label,
    panel,
    z = 50,
    title,
    onClose,
    children
  }: {
    label: string;
    panel: string;
    z?: number;
    title?: string;
    onClose: () => void;
    children: Snippet;
  } = $props();
</script>

<div class="dim" role="presentation" style:z-index={z} onclick={onClose}></div>
<div class="panel {panel}" role="dialog" aria-label={label} style:z-index={z + 1}>
  {#if title !== undefined}
    <div class="head">
      <span>{title}</span>
      <button class="x" title="close" onclick={onClose}>×</button>
    </div>
  {/if}
  {@render children()}
</div>

<style>
  .dim {
    position: fixed;
    inset: 0;
    background: rgba(5, 6, 8, 0.72);
  }
  .panel {
    position: fixed;
    display: flex;
    flex-direction: column;
    background: var(--raised);
    border: 1px solid var(--line2);
    border-radius: 10px;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.55);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    font: 700 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--head-color, var(--dim));
    flex: none;
  }
  .x {
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
    background: var(--hover);
    color: var(--tx);
  }
</style>
