<script lang="ts">
  // One transcript card. Code segments are protected from markdown
  // interpretation by renderMarkdown; reasoning is always open (the
  // collapse will come as a keybind, not a per-card toggle).
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
    void toolOut;
    void entry.args;
    void outputOpen;
    void argsOpen;
    if (el) heights.set(heightKey, el.offsetHeight);
  });

  // Provider text arrives with decorative leading/trailing newlines;
  // pre-wrap would render them as blank lines inside the card.
  const md = $derived(renderMarkdown((entry.text ?? '').trim()));
  // The user-facing tool output: for bash, the command is prepended to
  // the result, and the combined text is what gets truncated.
  const toolOut = $derived(
    entry.kind === 'tool'
      ? [
          entry.name === 'bash'
            ? safeArgs(entry.args)?.command ?? ''
            : '',
          entry.output ?? ''
        ]
          .filter(Boolean)
          .join('\n\n')
      : ''
  );
  // Sub-agent entries carry the raw payload as JSON; the block renders it
  // as plain key: value lines with the delimiters stripped.
  const sublines = $derived(
    entry.kind === 'subagent'
      ? structuredSub(entry.text ?? '')
      : []
  );
  // Tool outputs are long (command transcripts); a card under the cap shows
  // the full text with no expando.
  const preview = $derived(
    toolOut && toolOut.length > 200 ? toolOut.slice(0, 200) + ' …' : toolOut
  );
  const outputLong = $derived(toolOut.length > 200);
  let outputOpen = $state(false);
  // Long tool-call bodies collapse to the args line; a click opens the
  // full JSON under the tool name.
  let argsOpen = $state(false);
  // Note 5: a fully empty entry (a pure tool request whose payload has
  // not arrived) renders nothing, so no shell margin gap is left behind.
  const hasContent = $derived(
    Boolean(
      (entry.text ?? '').trim() ||
        entry.reasoning ||
        entry.kind === 'interrupted' ||
        (entry.kind === 'tool' && (Boolean(entry.args) || toolOut.length > 0))
    )
  );

  function fmt(n: number): string {
    return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
  }
  function structuredSub(text: string): string[] {
    const lines: string[] = [];
    const walk = (v: unknown, indent: number) => {
      if (Array.isArray(v)) {
        for (const x of v) walk(x, indent);
      } else if (v && typeof v === 'object') {
        for (const [k, x] of Object.entries(v as Record<string, unknown>)) {
          if (x && typeof x === 'object') {
            lines.push(' '.repeat(indent) + k + ':');
            walk(x, indent + 2);
          } else {
            lines.push(' '.repeat(indent) + k + ': ' + (x === null || x === undefined ? '' : String(x)));
          }
        }
      } else if (v !== null && v !== undefined && v !== '') {
        lines.push(' '.repeat(indent) + String(v));
      }
    };
    try {
      const p = JSON.parse(text);
      walk(p, 0);
    } catch {
      lines.push(text);
    }
    return lines;
  }
  function safeArgs(a: string | undefined): { command?: string } | null {
    if (!a) return null;
    try {
      return JSON.parse(a) as { command?: string };
    } catch {
      return null;
    }
  }
</script>

{#if hasContent}
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
      <button class="targs" type="button" title={entry.args} onclick={() => (argsOpen = !argsOpen)}>{entry.args}</button>
      <span class="tstatus" class:ok={entry.status === 'ok'} class:err={entry.status === 'error'}>
        {entry.status === 'ok' ? '✓' : entry.status === 'error' ? '✗' : '…'}
      </span>
    </div>
      {#if argsOpen && entry.args}
        <pre class="argsbody">{entry.args}</pre>
      {/if}
      {#if toolOut}
        {#if outputLong}
          <button class="expando" onclick={() => (outputOpen = !outputOpen)}>
            {outputOpen ? '▾ hide output' : '▸ full output (' + toolOut.length + ' chars)'}
          </button>
        {/if}
        <div class="out"><pre>{outputOpen ? toolOut : preview}</pre></div>
      {/if}
    </div>
  {:else}
    {#if entry.reasoning}
      <div class="reasonblock">
        <div class="reasontitle">reasoning</div>
        <div class="reason">
          {#if streaming}<span class="cursor"></span>
          {/if}{entry.reasoning.trim()}
        </div>
        {#if entry.usage}
          <div class="reasonmeta">{fmt(entry.usage.input_tokens)} in · {fmt(entry.usage.output_tokens)} out</div>
        {/if}
      </div>
    {/if}
    {#if entry.kind === 'om'}
      <div class="obsblock">
        <div class="obstitle">observation</div>
        <div class="obstext">{@html md}</div>
      </div>
    {:else if entry.kind === 'subagent'}
      <div class="subblock">
        <div class="subtitle">sub-agent</div>
        {#each sublines as line}
          <div class="subtext">{line}</div>
        {:else}
          <div class="subtext">{entry.text}</div>
        {/each}
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
    {:else if (entry.text && entry.text.trim()) || entry.kind === 'interrupted'}
      <div class="card" class:interrupted={entry.kind === 'interrupted'}>
        {#if entry.kind === 'interrupted'}
          <div class="intmark">⚡ interrupted</div>
        {/if}
        <div class="md">{@html md}</div>
      </div>
    {/if}
  {/if}
</div>
{/if}

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
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    cursor: pointer;
  }
  .argsbody {
    margin: 8px 0 0;
    padding: 8px 10px;
    background: #0c0e12;
    border: 1px solid var(--line);
    border-radius: 6px;
    font: 12px var(--mono);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 320px;
    overflow: auto;
    color: var(--dim);
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
    padding: 8px 10px 4px;
    font: 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--purple);
  }
  .reasonmeta {
    padding: 0 10px 6px;
    font: 10px var(--mono);
    color: #4d5462;
  }
  .reason {
    margin: 0;
    padding: 4px 10px 8px;
    color: #9a8fb8;
    font-size: 12px;
    white-space: pre-wrap;
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
  .obsblock {
    margin: 0 0 6px;
    border: 1px solid rgba(94, 200, 160, 0.16);
    border-left: 2px solid rgba(94, 200, 160, 0.35);
    border-radius: 8px;
    background: var(--panel);
  }
  .obstitle {
    display: block;
    width: 100%;
    padding: 8px 10px 4px;
    font: 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #5ec8a0;
  }
  .obstext {
    margin: 0;
    padding: 0 10px 8px;
    color: #7d948c;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .subblock {
    margin: 0 0 6px;
    border: 1px solid rgba(232, 180, 90, 0.16);
    border-left: 2px solid rgba(232, 180, 90, 0.35);
    border-radius: 8px;
    background: var(--panel);
  }
  .subtitle {
    display: block;
    width: 100%;
    padding: 8px 10px 4px;
    font: 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--amber);
  }
  .subtext {
    margin: 0;
    padding: 0 10px 8px;
    color: #b8a888;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
