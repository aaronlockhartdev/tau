<script lang="ts">
  // One transcript entry, V2 presentation: the presentation module maps
  // the entry to a presentable shape (lib/presentation.ts); this component
  // is the generic renderer over that shape — no per-kind branching. The
  // expanded card shells (tool chip, card) render in CardShell (#40).

  import { store } from '../lib/store.svelte';
  import { present, type ExpandSlot, type Shell } from '../lib/presentation';
  import type { Entry } from '../lib/protocol';
  import CardShell from './CardShell.svelte';

  let {
    entry,
    batch,
    heightKey,
    sourceLabel = '',
    parentLabel = '',
    turn = ''
  }: {
    entry: Entry;
    batch?: { index: number; size: number };
    heightKey: string;
    sourceLabel?: string;
    parentLabel?: string;
    turn?: 'you' | 'agent' | '';
  } = $props();

  let el = $state<HTMLDivElement | null>(null);

  // Expansion state: the store map is the persistence layer (the window
  // unmounts off-screen cards, so a purely local state would reset to
  // collapsed on remount — scrolling to the bottom collapsed every
  // expanded card); this local object is the reactive layer, seeded from
  // the map on mount. (Svelte 5.57 deep-proxies plain objects and arrays
  // in $state, but not Maps — the store map alone does not track.)
  const okey = (slot: string) => `${heightKey}:${slot}`;
  let open = $state({
    tool: !!store.entryOpen.get(okey('tool')),
    task: !!store.entryOpen.get(okey('task')),
    output: !!store.entryOpen.get(okey('output')),
    skill: !!store.entryOpen.get(okey('skill')),
    obs: !!store.entryOpen.get(okey('obs')),
    think: store.entryOpen.get(okey('think')) ?? false
  });
  function setOpen(slot: ExpandSlot) {
    const k = okey(slot);
    const next = !(store.entryOpen.get(k) ?? (slot === 'think' ? store.reasoningOpen : false));
    store.entryOpen.set(k, next);
    open[slot] = next;
  }
  // The 'r' keybind toggles every reasoning line at once (store global);
  // a click stores a per-entry override that then wins: the effect syncs
  // only entries without an override, so a clicked line keeps its state
  // across global flips.
  let thinkSynced = $state(false);
  $effect(() => {
    void store.reasoningOpen;
    if (store.reasoningOpen !== thinkSynced) {
      thinkSynced = store.reasoningOpen;
      open.think = store.entryOpen.get(okey('think')) ?? store.reasoningOpen;
    }
  });

  const p = $derived(present(entry, { sourceLabel, parentLabel }));

  function onKey(fn: () => void) {
    return (e: KeyboardEvent) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        fn();
      }
    };
  }
</script>

{#snippet shellView(shell: Shell)}
  {#if shell.kind === 'bubble'}
    <div class="tuser"><span class="bubble">{shell.text}</span></div>
  {:else if shell.kind === 'stline'}
    <div class="stline">
      <svg class="ic" width="13" height="13"><use href={`#${shell.icon}`}/></svg>{shell.label}
    </div>
  {:else if shell.kind === 'think'}
    <div
      class="think"
      class:open={open.think}
      role="button"
      tabindex="0"
      aria-expanded={open.think}
      onclick={() => setOpen('think')}
      ondblclick={(e) => {
        e.preventDefault();
        setOpen('think');
      }}
      onkeydown={onKey(() => setOpen('think'))}
    >
      <svg class="ic" width="13" height="13"><use href="#i-spark"/></svg>
      <span>{shell.label}</span>
      {#if shell.meta}
        <span class="meta">{shell.meta}</span>
      {/if}
    </div>
    {#if open.think}
      <div class="thinkbody md">{@html shell.body}</div>
    {/if}
  {:else}
    <CardShell shell={shell} open={open} setOpen={setOpen} onKey={onKey} />
  {/if}
{/snippet}
{#if p}
<div class="wrap" class:batch={!!batch} bind:this={el}>
  {#if batch && batch.index === 0}
    <div class="batch-mark">⊞ {batch.size} batched</div>
  {/if}
  {#if turn}
    <div class="thd">{turn}</div>
  {/if}
  {#each p.shells as shell, i (i)}
    {@render shellView(shell)}
  {/each}
</div>
{/if}

<style>
  .wrap {
    margin: 0 16px 8px;
  }
  /* A card in a parallel batch: a quiet connector line on the left ties the
     run together; the first card carries the count. */
  .wrap.batch {
    /* No margin-left override: the line sits at 16px, inline with the
       other cards' left edge. The padding clears the content from it. */
    padding-left: 10px;
    border-left: 2px solid var(--line);
  }
  .batch-mark {
    font: 10px var(--mono);
    color: var(--muted);
    /* Negative margin pulls the mark's left edge onto the line, so the
       line runs straight down under it. */
    margin: 0 0 3px -12px;
  }
  .thd {
    display: flex;
    align-items: center;
    gap: 8px;
    font: 10px var(--mono);
    letter-spacing: 0.08em;
    color: var(--faint);
    margin: 0 2px 8px;
  }
  .thd::after {
    content: '';
    flex: 1;
    border-top: 1px solid var(--line);
    opacity: 0.6;
  }
  .tuser {
    font-size: 13.5px;
  }
  .bubble {
    display: inline-block;
    background: var(--panel2);
    border: 1px solid var(--line2);
    border-radius: 10px;
    padding: 8px 12px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .think {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 0 2px;
    font: 12px var(--sans);
    color: var(--faint);
    cursor: pointer;
  }
  .think .ic {
    color: var(--purple);
    opacity: 0.7;
  }
  .think .meta {
    margin-left: auto;
    font: 10px var(--mono);
  }
  /* The state line: a quiet system line (the thinking line's look,
     without its interactivity). */
  .stline {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 0 2px;
    font: 12px var(--sans);
    color: var(--faint);
  }
  .stline .ic {
    color: var(--purple);
    opacity: 0.7;
  }
  /* The think line renders here, the card root in CardShell (its root node
     carries this component's scope, but the analyzer cannot see that), so
     the rule goes fully global. */
  :global(.think + .card2, .thinkbody + .card2) {
    margin-top: 10px;
  }
  .thinkbody {
    margin: 2px 2px 0;
    color: var(--dim);
    font-size: 12px;
    line-height: 1.5;
  }
  /* The renderer's output arrives via @html, outside scoping. */
  :global(pre.code) {
    padding: 10px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 12px/1.5 var(--mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    margin: 6px 0;
  }
  :global(.md p) {
    margin: 0;
  }
  :global(.md p + p) {
    margin-top: 6px;
  }
  :global(.md code) {
    font: 12px var(--mono);
    color: var(--acc);
  }
  :global(.md strong) {
    color: #fff;
  }
  :global(.md em) {
    color: var(--dim);
  }
  :global(.md h1),
  :global(.md h2),
  :global(.md h3) {
    font-weight: 700;
    margin: 8px 0 4px;
  }
  :global(.md h1) {
    font-size: 16.5px;
  }
  :global(.md h2) {
    font-size: 15.5px;
  }
  :global(.md h3) {
    font-size: 14.5px;
  }
  :global(.md ul),
  :global(.md ol) {
    margin: 4px 0;
    padding-left: 18px;
  }
  :global(.md li) {
    margin: 2px 0;
  }
  :global(.md a) {
    color: var(--acc);
  }
  :global(.md table) {
    border-collapse: collapse;
    margin: 6px 0;
  }
  :global(.md th),
  :global(.md td) {
    border: 1px solid var(--line);
    padding: 4px 8px;
  }
</style>
