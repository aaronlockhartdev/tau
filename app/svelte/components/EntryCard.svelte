<script lang="ts">
  // One transcript entry, V2 presentation: the user entry is a bubble; the
  // other kinds are unified cards whose header is an icon plus a quiet
  // label; reasoning is a cardless grey "thinking" line. Tool calls are a
  // chip row that expands to structured key/value args plus the output.
  // The card measures itself on mount and on content change; the
  // transcript windowing consumes the measurement through `heights`.

  import { onMount } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { md as renderMarkdown } from '../lib/markdown';
  import type { Entry } from '../lib/protocol';

  let {
    entry,
    heightKey,
    heights,
    sourceLabel = '',
    turn = ''
  }: {
    entry: Entry;
    heightKey: string;
    heights: Map<string, number>;
    sourceLabel?: string;
    turn?: 'you' | 'agent' | '';
  } = $props();

  let el = $state<HTMLDivElement | null>(null);

  onMount(() => {
    if (el) heights.set(heightKey, el.offsetHeight);
  });

  // live streams grow the card; re-measure as content changes.
  let thinkOpen = $state(false);
  let thinkSynced = $state(false);
  // The 'r' keybind toggles every reasoning line at once (store global);
  // a click toggles only this one. The effect syncs the local state when
  // the global changes, so the keybind wins on its next press.
  $effect(() => {
    void store.reasoningOpen;
    if (store.reasoningOpen !== thinkSynced) {
      thinkOpen = store.reasoningOpen;
      thinkSynced = store.reasoningOpen;
    }
  });
  let toolOpen = $state(false);
  let outputOpen = $state(false);
  $effect(() => {
    void entry.text;
    void entry.reasoning;
    void entry.output;
    void toolOut;
    void entry.args;
    void outputOpen;
    void toolOpen;
    void thinkOpen;
    if (el) heights.set(heightKey, el.offsetHeight);
  });

  // Provider text arrives with decorative leading/trailing newlines;
  // pre-wrap would render them as blank lines inside the card.
  const md = $derived(renderMarkdown((entry.text ?? '').trim()));
  const reasonMd = $derived(entry.reasoning ? renderMarkdown(entry.reasoning.trim()) : '');

  // The user-facing tool output: for bash, the command is prepended to
  // the result, and the combined text is what gets truncated.
  const toolOut = $derived(
    entry.kind === 'tool'
      ? [
          entry.name === 'bash'
            ? parseArgs(entry.args)?.command ?? ''
            : '',
          entry.output ?? ''
        ]
          .filter(Boolean)
          .join('\n\n')
      : ''
  );
  // The chip's one-line summary: the argument that names the operation.
  const toolSummary = $derived.by(() => {
    if (entry.kind !== 'tool') return '';
    const a = parseArgs(entry.args);
    if (!a) return entry.args ?? '';
    const pick = (a as Record<string, unknown>);
    const v =
      pick.command ?? pick.file_path ?? pick.task ?? pick.query ?? pick.task_id ?? pick.title ?? pick.message;
    const s = typeof v === 'string' ? v : '';
    return s ? (s.length > 80 ? s.slice(0, 80) + '…' : s) : entry.args ?? '';
  });
  // Expanded args as key/value lines; non-scalar values collapse to JSON.
  const toolKv = $derived.by((): Array<[string, string]> => {
    if (entry.kind !== 'tool') return [];
    const a = parseArgs(entry.args);
    if (!a) return entry.args ? [['', entry.args]] : [];
    return Object.entries(a as Record<string, unknown>).map(([k, v]) => [
      k,
      typeof v === 'object' && v !== null ? JSON.stringify(v) : String(v)
    ]);
  });
  const toolIcon = $derived(
    entry.kind === 'tool'
      ? (['read', 'write', 'edit'].includes(entry.name ?? '')
          ? 'i-file'
          : entry.name?.startsWith('subagent_') || entry.name === 'parent_notify'
            ? 'i-bot'
            : entry.name === 'recall'
              ? 'i-search'
              : entry.name?.startsWith('task_')
                ? 'i-check'
                : 'i-term')
      : 'i-term'
  );

  const preview = $derived(
    toolOut && toolOut.length > 200 ? toolOut.slice(0, 200) + ' …' : toolOut
  );
  const outputLong = $derived(toolOut.length > 200);

  // The /skill: block (ticket #28): the body collapses to the usual
  // 200-char preview with an expando (the tool-output mechanism).
  let skillOpen = $state(false);
  const skillLong = $derived((entry.text ?? '').length > 200);
  const skillPreview = $derived(
    skillLong ? (entry.text ?? '').slice(0, 200) + ' …' : (entry.text ?? '')
  );

  // Sub-agent entries carry the raw payload as JSON; the card renders it
  // as plain key: value lines with the delimiters stripped.
  const sublines = $derived(
    entry.kind === 'subagent'
      ? structuredSub(entry.text ?? '')
      : []
  );

  // A fully empty entry (a pure tool request whose payload has
  // not arrived) renders nothing, so no shell margin gap is left behind.
  const hasContent = $derived(
    Boolean(
      (entry.text ?? '').trim() ||
        entry.reasoning ||
        entry.kind === 'interrupted' ||
        (entry.kind === 'tool' && (Boolean(entry.args) || toolOut.length > 0))
    )
  );

  function parseArgs(a: string | undefined): { command?: string } | null {
    if (!a) return null;
    try {
      return JSON.parse(a) as { command?: string };
    } catch {
      return null;
    }
  }
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
  function onKey(fn: () => void) {
    return (e: KeyboardEvent) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        fn();
      }
    };
  }
</script>

{#if hasContent}
<div class="wrap" bind:this={el}>
  {#if turn}
    <div class="thd">{turn}</div>
  {/if}
  {#if entry.kind === 'user'}
    {#if entry.skill}
      <div class="card2">
        <div class="hd skill"><svg class="ic" width="13" height="13"><use href="#i-book"/></svg>skill · {entry.skill.name}</div>
        <div class="txt2 dim">{skillOpen ? (entry.text ?? '') : skillPreview}</div>
        {#if skillLong}
          <button class="expando" onclick={() => (skillOpen = !skillOpen)}>
            {skillOpen ? '▾ hide' : '▸ full (' + (entry.text ?? '').length + ' chars)'}
          </button>
        {/if}
      </div>
    {:else if entry.source}
      <div class="card2">
        <div class="hd sub"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>sub-agent{sourceLabel ? ` · ${sourceLabel}` : ''}</div>
        <div class="txt2 dim">{entry.text}</div>
      </div>
    {:else}
      <div class="tuser"><span class="bubble">{entry.text}</span></div>
    {/if}
  {:else if entry.kind === 'tool'}
    <div class="tool" class:open={toolOpen}>
      <div
        class="chip"
        role="button"
        tabindex="0"
        onclick={() => (toolOpen = !toolOpen)}
        onkeydown={onKey(() => (toolOpen = !toolOpen))}
      >
        <svg class="ic" width="13" height="13"><use href={`#${toolIcon}`}/></svg>
        <span class="nm">{entry.name}</span>
        <span class="sum">{toolSummary}</span>
        <span class="st {entry.status === 'ok' ? 'ok' : entry.status === 'error' ? 'err' : 'pend'}">
          {entry.status === 'ok' ? '✓' : entry.status === 'error' ? '✗' : '…'}
        </span>
        <span class="caret">▾</span>
      </div>
      {#if toolOpen}
        <div class="x">
          {#each toolKv as [k, v]}
            <div class="kv"><span class="k">{k}</span><span class="v">{v}</span></div>
          {/each}
          {#if toolOut}
            <div class="out"><pre>{outputOpen ? toolOut : preview}</pre></div>
            {#if outputLong}
              <button class="expando" onclick={() => (outputOpen = !outputOpen)}>
                {outputOpen ? '▾ hide output' : '▸ full output (' + toolOut.length + ' chars)'}
              </button>
            {/if}
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    {#if entry.reasoning}
      <div
        class="think"
        class:open={thinkOpen}
        role="button"
        tabindex="0"
        aria-expanded={thinkOpen}
        onclick={() => (thinkOpen = !thinkOpen)}
        onkeydown={onKey(() => (thinkOpen = !thinkOpen))}
      >
        <svg class="ic" width="13" height="13"><use href="#i-spark"/></svg>
        <span>thinking</span>
        {#if entry.usage}
          <span class="meta">{fmt(entry.usage.input_tokens)} in · {fmt(entry.usage.output_tokens)} out</span>
        {/if}
      </div>
      {#if thinkOpen}
        <div class="thinkbody md">{@html reasonMd}</div>
      {/if}
    {/if}
    {#if entry.kind === 'om'}
      <div class="card2">
        <div class="hd obs"><svg class="ic" width="13" height="13"><use href="#i-book"/></svg>observation</div>
        <div class="txt2 dim">{@html md}</div>
      </div>
    {:else if entry.kind === 'subagent'}
      <div class="card2">
        <div class="hd sub"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>sub-agent</div>
        {#each sublines as line}
          <div class="txt2 dim">{line}</div>
        {:else}
          <div class="txt2 dim">{entry.text}</div>
        {/each}
      </div>
    {:else if entry.kind === 'spawn-snapshot'}
      <div class="card2">
        <div class="hd sub"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>spawn snapshot</div>
        <div class="txt2">{@html md}</div>
      </div>
    {:else if entry.kind === 'system'}
      <div class="card2">
        <div class="hd sys"><svg class="ic" width="13" height="13"><use href="#i-term"/></svg>system</div>
        <div class="txt2 dim">{@html md}</div>
      </div>
    {:else if (entry.text && entry.text.trim()) || entry.kind === 'interrupted'}
      <div class="card2" class:interrupted={entry.kind === 'interrupted'}>
        {#if entry.kind === 'interrupted'}
          <div class="intmark">⚡ interrupted</div>
        {/if}
        <div class="txt2">{@html md}</div>
      </div>
    {/if}
  {/if}
</div>
{/if}

<style>
  .wrap {
    margin: 0 16px 10px;
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
  .hd .ic {
    color: var(--purple);
  }
  .hd.sub .ic {
    color: var(--amber);
  }
  .hd.skill .ic,
  .hd.obs .ic {
    color: var(--green);
  }
  .txt2 {
    white-space: pre-wrap;
    word-break: break-word;
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
  .thinkbody {
    margin: 2px 2px 0;
    color: var(--dim);
    font-size: 12px;
    line-height: 1.5;
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
  .kv .v {
    color: var(--dim);
    word-break: break-word;
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
    margin-bottom: 4px;
  }
  /* The renderer's output arrives via @html, outside scoping. */
  :global(.md pre) {
    padding: 10px;
    background: var(--bg);
    border: 1px solid var(--line);
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
    color: var(--dim);
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
