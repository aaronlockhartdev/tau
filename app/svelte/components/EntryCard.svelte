<script lang="ts">
  // One transcript card. Code segments are protected from markdown
  // interpretation by renderMarkdown; reasoning is viewable and
  // collapsible, visible by default (gui.reasoning_visible default).
  // The card measures itself on mount and on content change; the
  // transcript windowing consumes the measurement through `heights`.

  import { onMount } from 'svelte';
  import { md as renderMarkdown } from '../lib/markdown';
  import type { Entry } from '../lib/protocol';

  let {
    entry,
    heightKey,
    heights,
    streaming = false
  }: {
    entry: Entry;
    heightKey: string;
    heights: Map<string, number>;
    streaming?: boolean;
  } = $props();

  let el = $state<HTMLDivElement | null>(null);

  onMount(() => {
    if (el) heights.set(heightKey, el.offsetHeight);
  });

  // live streams grow the card; re-measure as content changes.
  $effect(() => {
    void entry.text;
    void entry.reasoning;
    void entry.output;
    void outputOpen;
    if (el) heights.set(heightKey, el.offsetHeight);
  });

  // Provider text arrives with decorative leading/trailing newlines;
  // pre-wrap would render them as blank lines inside the card.
  const md = $derived(renderMarkdown((entry.text ?? '').trim()));
  // Tool outputs are long (command transcripts); the collapsed card caps the
  // preview at 200 chars.
  const preview = $derived(
    entry.output && entry.output.length > 200 ? entry.output.slice(0, 200) + ' …' : entry.output ?? ''
  );
  let reasoningOpen = $state(true);
  let outputOpen = $state(false);

  function fmt(n: number): string {
    return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
  }
</script>

<div class="wrap" bind:this={el}>
  {#if entry.kind === 'user'}
    <div class="card user">
      <div class="userbubble">{entry.text}</div>
    </div>
  {:else if entry.kind === 'tool'}
    <div class="card tool">
    <div class="toolrow">
      <span class="ticon">⚒</span>
      <span class="tname">{entry.name}</span>
      <span class="targs" title={entry.args}>{entry.args}</span>
      <span class="tstatus" class:ok={entry.status === 'ok'} class:err={entry.status === 'error'}>
        {entry.status === 'ok' ? '✓' : entry.status === 'error' ? '✗' : '…'}
      </span>
    </div>
      {#if entry.output}
        <button class="expando" onclick={() => (outputOpen = !outputOpen)}>
          {outputOpen ? '▾ hide output' : '▸ full output (' + entry.output.length + ' chars)'}
        </button>
        <div class="out"><pre>{outputOpen ? entry.output : preview}</pre></div>
      {/if}
    </div>
  {:else}
    {#if entry.reasoning}
      <div class="reasonblock" class:open={reasoningOpen}>
        <button class="reasontitle" onclick={() => (reasoningOpen = !reasoningOpen)}>
          {reasoningOpen ? '▾' : '▸'} reasoning
        </button>
        {#if reasoningOpen}
          <div class="reason">
            {#if streaming}<span class="cursor"></span>
            {/if}{entry.reasoning.trim()}
          </div>
        {/if}
      </div>
    {/if}
    {#if entry.kind === 'om'}
      <div class="card">
        <div class="klabel">observation log</div>
        <div class="md">{@html md}</div>
      </div>
    {:else if entry.kind === 'spawn-snapshot'}
      <div class="card">
        <div class="klabel">spawn snapshot</div>
        <div class="md">{@html md}</div>
      </div>
    {:else if entry.kind === 'system'}
      <div class="card">
        <div class="klabel">system</div>
        <div class="md">{@html md}</div>
      </div>
    {:else if entry.text || entry.usage || entry.kind === 'interrupted'}
      <div class="card" class:interrupted={entry.kind === 'interrupted'}>
        {#if entry.kind === 'interrupted'}
          <div class="intmark">⚡ interrupted</div>
        {/if}
        <div class="md">{@html md}</div>
        {#if entry.usage}
          <div class="meta">{fmt(entry.usage.input_tokens)} in · {fmt(entry.usage.output_tokens)} out</div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .wrap {
    margin: 0 16px 10px;
  }
  .card {
    margin: 0;
    padding: 10px 14px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    font-size: 13.5px;
    line-height: 1.45;
  }
  .card.user {
    background: var(--panel2);
    border-color: #2e3340;
  }
  .card.tool {
    background: transparent;
    border-color: #22262f;
    padding: 8px 12px;
  }
  .userbubble {
    white-space: pre-wrap;
  }
  .toolrow {
    display: flex;
    align-items: center;
    gap: 8px;
    font: 12px var(--mono);
  }
  .ticon {
    color: var(--dim);
  }
  .tname {
    color: var(--acc);
    font-weight: 600;
  }
  .targs {
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .tstatus {
    color: var(--dim);
  }
  .tstatus.ok {
    color: var(--green);
  }
  .tstatus.err {
    color: var(--red);
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
  .reasonblock {
    margin: 0 0 6px;
    border: 1px solid rgba(181, 140, 255, 0.16);
    border-left: 2px solid rgba(181, 140, 255, 0.35);
    border-radius: 8px;
    background: var(--panel);
  }
  .reasontitle {
    display: block;
    width: 100%;
    padding: 4px 10px;
    text-align: left;
    background: transparent;
    border: 0;
    cursor: pointer;
    font: 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--purple);
  }
  .reasontitle:hover {
    color: #b58cff;
  }
  .reason {
    margin: 0;
    padding: 4px 10px 8px;
    color: #9a8fb8;
    font-size: 12px;
    white-space: pre-wrap;
    max-height: 160px;
    overflow-y: auto;
  }
  .cursor {
    display: inline-block;
    width: 6px;
    height: 12px;
    background: var(--acc);
    animation: pulse 1s infinite;
    vertical-align: text-bottom;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
  .out {
    margin-top: 6px;
  }
  .out pre {
    margin: 0;
    padding: 8px;
    background: #0c0e12;
    border-radius: 6px;
    font: 11.5px/1.5 var(--mono);
    overflow-x: auto;
    max-height: 240px;
    white-space: pre-wrap;
  }
  .klabel {
    font: 9.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--purple);
    margin-bottom: 3px;
  }
  .intmark {
    font: 11px var(--mono);
    color: var(--amber);
    margin-bottom: 4px;
  }
  .meta {
    margin-top: 6px;
    font: 10.5px var(--mono);
    color: #4d5462;
  }
  .md {
    white-space: pre-wrap;
    word-break: break-word;
  }
  /* The renderer's output arrives via @html, outside scoping. */
  :global(.md pre) {
    padding: 10px;
    background: #0c0e12;
    border-radius: 6px;
    font: 12px/1.5 var(--mono);
    overflow-x: auto;
    margin: 6px 0;
  }
  :global(.md pre code) {
    color: var(--tx);
  }
  :global(.md code) {
    font: 12px var(--mono);
    color: var(--acc);
  }
  :global(.md b) {
    color: #fff;
  }
  :global(.md i) {
    color: #9a94b0;
  }
  :global(.md .mh) {
    font-weight: 700;
    font-size: 14.5px;
    margin: 8px 0 4px;
  }
  :global(.md .mh2) {
    font-weight: 700;
    font-size: 15.5px;
    margin: 8px 0 4px;
  }
  :global(.md .mi) {
    padding-left: 14px;
    position: relative;
  }
  :global(.md .mi::before) {
    content: '·';
    position: absolute;
    left: 2px;
  }
  :global(.md .lk) {
    color: var(--acc);
  }
</style>
