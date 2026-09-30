<script module>
  // Test-handle registry: each mounted mock registers a scroll driver so
  // a test can invoke the onscroll prop (the real virtua drives it from
  // webview scrolling, which jsdom cannot produce).
  export const drives: Array<(offset: number) => void> = [];
</script>

<script lang="ts">
  // Test double for virtua/svelte's Virtualizer: renders every item (no
  // windowing) and exposes the handle methods Transcript's pin/scroll
  // logic reads.
  import { onDestroy } from 'svelte';
  import type { Snippet } from 'svelte';

  let {
    scrollRef = undefined,
    data = [],
    getKey = (d: unknown) => String(d),
    bufferSize = 0,
    onscroll = undefined,
    children
  }: {
    scrollRef?: HTMLElement | undefined;
    data: unknown[];
    getKey?: (d: unknown) => string;
    bufferSize?: number;
    onscroll?: (offset: number) => void;
    children: Snippet<[unknown, number]>;
  } = $props();

  const drive = (offset: number): void => onscroll?.(offset);
  drives.push(drive);
  onDestroy(() => {
    const i = drives.indexOf(drive);
    if (i >= 0) drives.splice(i, 1);
  });

  // Handle surface (bind:this) — inert in tests.
  export function scrollToIndex(): void {}
  export function getScrollSize(): number {
    return 0;
  }
  export function getViewportSize(): number {
    return 0;
  }
  export function findItemIndex(): number {
    return 0;
  }
</script>

{#each data as d, i (getKey(d))}
  {@render children(d, i)}
{/each}
