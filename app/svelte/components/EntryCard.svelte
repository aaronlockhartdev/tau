<script lang="ts">
  // One transcript entry, V2 presentation: the presentation module maps
  // the entry to a presentable shape (lib/presentation.ts); this component
  // is the generic renderer over that shape — no per-kind branching.

  import { store } from '../lib/store.svelte';
  import {
    present,
    type ExpandSlot,
    type KvRow,
    type Section,
    type Shell
  } from '../lib/presentation';
  import type { Entry } from '../lib/protocol';

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

{#snippet kvRows(rows: KvRow[])}
  {#each rows as row (row.k)}
    {#if row.k}
      <div class="kv"><span class="k">{row.k}:</span></div>
    {/if}
    {#each row.lines as ln (ln)}
      <div class="kv sub"><span class="v">{ln}</span></div>
    {/each}
  {/each}
{/snippet}
{#snippet sectionView(s: Section, isOpen: boolean)}
  {#if s.type === 'md'}
    {#if s.label}
      <div class="obs-seclbl">{s.label}</div>
    {/if}
    {#if s.cls === 'thinkbody'}
      <div class="thinkbody md">{@html s.html}</div>
    {:else}
      <div class="txt2 md" class:dim={s.dim === true}>{@html s.html}</div>
    {/if}
  {:else if s.type === 'kv'}
    {#if s.block}
      <div class="kvblock">{@render kvRows(s.rows)}</div>
    {:else}
      {@render kvRows(s.rows)}
    {/if}
  {:else if s.type === 'text'}
    <div class="txt2" class:dim={s.dim === true}>
      {isOpen || !s.whenClosed ? s.text : s.whenClosed}
    </div>
  {:else}
    {#if s.label}
      <div class="obs-seclbl">{s.label}</div>
    {/if}
    <pre class="obs-pre">{s.text}</pre>
  {/if}
{/snippet}
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
  {:else if shell.kind === 'tool'}
    <div class="tool" class:open={open.tool}>
      <div
        class="chip"
        role="button"
        tabindex="0"
        onclick={() => setOpen('tool')}
        ondblclick={(e) => {
          e.preventDefault();
          setOpen('tool');
        }}
        onkeydown={onKey(() => setOpen('tool'))}
      >
        <svg class="ic" width="13" height="13"><use href={`#${shell.icon}`}/></svg>
        <span class="nm">{shell.label}</span>
        <span class="sum">{shell.summary}</span>
        <span class="st {shell.status === 'ok' ? 'ok' : shell.status === 'error' ? 'err' : 'pend'}">
          {#if shell.status === 'running'}
            <span class="spin"></span>
          {:else}
            {shell.status === 'ok' ? '✓' : '✗'}
          {/if}
        </span>
        <span class="caret">▾</span>
      </div>
      {#if open.tool}
        <div class="x">
          {@render kvRows(shell.kv)}
          {#if shell.output}
            <div class="out"><pre>{open.output ? shell.output.text : shell.output.closed}</pre></div>
            {#if shell.output.long}
              <button class="expando" onclick={() => setOpen('output')}>
                {open.output ? '▾ hide output' : `▸ full output (${shell.output.text.length} chars)`}
              </button>
            {/if}
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    {@const isOpen = shell.openKey ? open[shell.openKey] : false}
    <div
      class="card2"
      class:obs={shell.cls === 'obs'}
      class:interrupted={shell.cls === 'interrupted'}
      class:collapsed={shell.collapseWhenClosed && !isOpen}
    >
      {#if shell.header}
        {@const h = shell.header}
        {@const hopen = h.open}
        {#if hopen !== null}
          <div
            class="hd {h.cls}"
            role="button"
            tabindex="0"
            aria-expanded={isOpen}
            onclick={h.singleClick ? () => setOpen(hopen) : undefined}
            ondblclick={(e) => {
              e.preventDefault();
              setOpen(hopen);
            }}
            onkeydown={onKey(() => setOpen(hopen))}
          >
            <svg class="ic" width="13" height="13"><use href={`#${h.icon}`}/></svg>{h.label}
            {#if h.meta}
              <span class="meta">{h.meta}</span>
            {/if}
            {#if h.caret}
              <span class="caret">{isOpen ? '▾' : '▸'}</span>
            {/if}
          </div>
        {:else}
          <div class="hd {h.cls}">
            <svg class="ic" width="13" height="13"><use href={`#${h.icon}`}/></svg>{h.label}
            {#if h.meta}
              <span class="meta">{h.meta}</span>
            {/if}
            {#if h.caret}
              <span class="caret">{isOpen ? '▾' : '▸'}</span>
            {/if}
          </div>
        {/if}
      {/if}
      {#each shell.sections as s (s)}
        {@render sectionView(s, isOpen)}
      {/each}
      {#if shell.reveal && isOpen}
        {#if shell.reveal.group}
          <div class="obs-details">
            {#each shell.reveal.sections as s (s)}
              {@render sectionView(s, true)}
            {/each}
          </div>
        {:else}
          {#each shell.reveal.sections as s (s)}
            {@render sectionView(s, true)}
          {/each}
        {/if}
      {/if}
      {#if shell.expando}
        {@const exp = shell.expando}
        <button class="expando" onclick={() => setOpen(exp.openKey)}>
          {open[exp.openKey] ? exp.whenOpen : exp.whenClosed}
        </button>
      {/if}
      {#if shell.intmark}
        <div class="intmark">⚡ interrupted</div>
      {/if}
    </div>
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
  .card2 {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 10px 14px;
    font-size: 13.5px;
    line-height: 1.45;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
  }
  .hd {
    display: flex;
    align-items: center;
    gap: 7px;
    font: 10.5px var(--mono);
    letter-spacing: 0.04em;
    color: var(--dim);
    margin-bottom: 6px;
  }
  .card2.collapsed .hd {
    margin-bottom: 0;
  }
  .hd .ic {
    color: var(--purple);
  }
  .hd.sub .ic {
    color: var(--amber);
  }
  .hd.par .ic {
    color: var(--acc);
  }
  .hd.skill .ic,
  .hd.obs .ic {
    color: var(--green);
  }
  .txt2 {
    white-space: pre-wrap;
    word-break: break-word;
  }
  /* Rendered markdown is HTML: source newlines between tags are not
     meaningful, so collapse them (pre-wrap would turn each into a line
     break -- the trailing blank line and the extra space around <br>). */
  .txt2.md {
    white-space: normal;
  }
  .txt2.dim {
    color: var(--dim);
    font-size: 12.5px;
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
  .think + .card2,
  .thinkbody + .card2 {
    margin-top: 10px;
  }
  .thinkbody {
    margin: 2px 2px 0;
    color: var(--dim);
    font-size: 12px;
    line-height: 1.5;
  }
  .hd .meta {
    margin-left: auto;
    margin-right: 6px;
    font: 10px var(--mono);
    color: var(--faint);
  }
  .obs-details {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--line);
  }
  .obs-seclbl {
    margin-top: 8px;
    font: 10px var(--mono);
    letter-spacing: 0.04em;
    color: var(--faint);
  }
  .obs-pre {
    margin: 4px 0 0;
    padding: 8px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 11.5px/1.5 var(--mono);
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--dim);
  }
  .tool .chip {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 7px 10px;
    cursor: pointer;
  }
  .tool .chip .ic {
    color: var(--dim);
  }
  .tool .chip .nm {
    color: var(--tx);
    font-weight: 600;
    font: 12px var(--mono);
  }
  .tool .chip .sum {
    flex: 1;
    color: var(--faint);
    font: 11px var(--mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tool .chip .st {
    font-size: 12px;
    color: var(--faint);
  }
  .tool .chip .st.ok {
    color: var(--green);
  }
  .tool .chip .st.err {
    color: var(--red);
  }
  .tool .chip .st .spin {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 2px solid var(--line2);
    border-top-color: var(--acc);
    border-radius: 50%;
    animation: toolspin 0.9s linear infinite;
  }
  @keyframes toolspin {
    to {
      transform: rotate(360deg);
    }
  }
  .tool .chip .caret {
    color: var(--faint);
    font-size: 10px;
    transition: transform 0.12s;
  }
  .tool.open .chip .caret {
    transform: rotate(90deg);
  }
  .tool .x {
    margin: 6px 0 0 18px;
    padding: 8px 10px;
    border-left: 2px solid var(--line2);
  }
  .kv {
    display: grid;
    grid-template-columns: 110px 1fr;
    gap: 2px 10px;
    font: 11.5px var(--mono);
    margin-bottom: 6px;
  }
  .kv .k {
    color: var(--faint);
  }
  .hd .caret {
    margin-left: auto;
    color: var(--faint);
    font-size: 10px;
  }
  .hd[role='button'] {
    cursor: pointer;
  }
  .kv .v {
    color: var(--dim);
    word-break: break-word;
  }
  .kv.sub {
    grid-template-columns: 1fr;
    margin-left: 14px;
  }
  .kv.sub .v {
    white-space: pre-wrap;
  }
  .out {
    margin-top: 6px;
  }
  .out pre {
    margin: 0;
    padding: 8px;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 11.5px/1.5 var(--mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--dim);
  }
  .expando {
    display: inline-block;
    margin-top: 6px;
    font: 11px var(--mono);
    color: var(--dim);
  }
  .expando:hover {
    color: var(--tx);
  }
  .intmark {
    font: 11px var(--mono);
    color: var(--amber);
    margin-top: 6px;
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
