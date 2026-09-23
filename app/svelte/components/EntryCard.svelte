<script lang="ts">
  // One transcript entry, V2 presentation: the user entry is a bubble; the
  // other kinds are unified cards whose header is an icon plus a quiet
  // label; reasoning is a cardless grey "thinking" line. Tool calls are a
  // chip row that expands to structured key/value args plus the output.
  // The card reports its rendered height on mount and on content change;
  // the transcript owns the heights map and the track relayout.

  import { onMount } from 'svelte';
  import { store } from '../lib/store.svelte';
  import { md as renderMarkdown, splitJsonPayload, argsLines, valueLinesOf } from '../lib/markdown';
  import type { Entry } from '../lib/protocol';

  let {
    entry,
    heightKey,
    report,
    sourceLabel = '',
    parentLabel = '',
    turn = ''
  }: {
    entry: Entry;
    heightKey: string;
    report?: (h: number) => void;
    sourceLabel?: string;
    parentLabel?: string;
    turn?: 'you' | 'agent' | '';
  } = $props();

  let el = $state<HTMLDivElement | null>(null);

  function measure() {
    if (el && report) report(el.offsetHeight);
  }

  onMount(() => {
    measure();
  });

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
    think: store.entryOpen.get(okey('think')) ?? false
  });
  function setOpen(slot: 'tool' | 'task' | 'output' | 'skill' | 'think') {
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
  let thinkOpen = $derived(open.think);
  let toolOpen = $derived(open.tool);
  let taskOpen = $derived(open.task);
  let outputOpen = $derived(open.output);
  let skillOpen = $derived(open.skill);
  $effect(() => {
    void entry.text;
    void entry.reasoning;
    void entry.output;
    void toolOut;
    void open.output;
    void open.tool;
    void open.task;
    void open.think;
    void thinkSynced;
    measure();
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
  // Expanded args as structured key/value lines (nested values as
  // indented bullets, not a raw JSON blob).
  const toolKv = $derived.by((): Array<{ k: string; lines: string[] }> => {
    if (entry.kind !== 'tool') return [];
    const a = parseArgs(entry.args);
    if (!a) return entry.args ? [{ k: '', lines: [entry.args] }] : [];
    return argsLines(a as Record<string, unknown>);
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
  const skillLong = $derived((entry.text ?? '').length > 200);
  const skillPreview = $derived(
    skillLong ? (entry.text ?? '').slice(0, 200) + ' …' : (entry.text ?? '')
  );

  // Sub-agent entries carry the raw payload as JSON; the card renders it
  // with the shared structured kv rows (field name on its own line with a
  // colon, value below). State transitions are the exception: one quiet
  // line, no card shell, no kv body — the record is the discriminant plus
  // the non-output variant fields, and the output lives in the notify
  // record (the report), which keeps its card. In a child's own session
  // these are its own lifecycle records — not amber sub-agent cards (the
  // amber card is the parent's view of a child).
  const subKv = $derived.by((): Array<{ k: string; lines: string[] }> => {
    if (entry.kind !== 'subagent') return [];
    try {
      const p = JSON.parse(entry.text ?? '') as Record<string, unknown>;
      return Object.entries(p).map(([k, v]) => ({ k, lines: valueLinesOf(v, 1) }));
    } catch {
      return entry.text ? [{ k: '', lines: [entry.text] }] : [];
    }
  });
  // The state line: `state: done`, `state: idle · {waiting_on}`,
  // `state: failed · {reason}`, `state: stopped`.
  const subState = $derived.by(() => {
    if (entry.kind !== 'subagent') return '';
    try {
      const p = JSON.parse(entry.text ?? '') as Record<string, any>;
      if (p.event !== 'state') return '';
      const st = typeof p.state === 'string' ? p.state : (p.state?.state ?? '');
      if (st === 'idle' && p.waiting_on) return `state: ${st} · ${p.waiting_on}`;
      if (st === 'failed' && p.reason) return `state: ${st} · ${p.reason}`;
      return `state: ${st}`;
    } catch {
      return '';
    }
  });
  const subLabel = $derived.by(() => {
    if (entry.kind !== 'subagent') return '';
    try {
      const p = JSON.parse(entry.text ?? '') as Record<string, any>;
      if (p.event === 'spawn') return 'spawn';
      if (p.event === 'notify') return 'reported to parent';
      return 'sub-agent';
    } catch {
      return 'sub-agent';
    }
  });

  // A sub-agent's message (child → parent) and the parent's message to a
  // child: prose with a JSON payload, rendered as kv lines the way an
  // expanded tool call does.
  const msg = $derived(
    entry.kind === 'user' && (entry.source || parentLabel)
      ? splitJsonPayload(entry.text ?? '')
      : null
  );
  // Task lifecycle records (created / started / evidence / finished / ...)
  // carry the raw payload as JSON; render it as a quiet system card with
  // an event label and structured kv lines (like a tool's args).
  const taskKv = $derived.by((): Array<{ k: string; lines: string[] }> => {
    if (entry.kind !== 'task') return [];
    try {
      const p = JSON.parse(entry.text ?? '') as Record<string, unknown>;
      return Object.entries(p)
        .filter(([k]) => k !== 'event')
        .map(([k, v]) => ({ k, lines: valueLinesOf(v, 1) }));
    } catch {
      return entry.text ? [{ k: '', lines: [entry.text] }] : [];
    }
  });
  const taskLabel = $derived.by(() => {
    if (entry.kind !== 'task') return '';
    try {
      const p = JSON.parse(entry.text ?? '') as Record<string, any>;
      if (p.event === 'created') return `task: ${p.title ?? p.id}`;
      if (p.event === 'started') return `task started · ${p.id}`;
      if (p.event === 'evidence') return `evidence · ${p.evidence?.criterion ?? p.id}`;
      if (p.event === 'finished') return `task done · ${p.id}`;
      if (p.event === 'blocked') return `task blocked · ${p.id}`;
      if (p.event === 'assigned') return `task assigned · ${p.id}`;
      return p.event ?? 'task';
    } catch {
      return 'task';
    }
  });

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
  function onKey(fn: () => void) {
    return (e: KeyboardEvent) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        fn();
      }
    };
  }
</script>

{#snippet kvRows(rows: Array<{ k: string; lines: string[] }>)}
  {#each rows as row (row.k)}
    {#if row.k}
      <div class="kv"><span class="k">{row.k}:</span></div>
    {/if}
    {#each row.lines as ln (ln)}
      <div class="kv sub"><span class="v">{ln}</span></div>
    {/each}
  {/each}
{/snippet}
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
          <button class="expando" onclick={() => setOpen('skill')}>
            {skillOpen ? '▾ hide' : '▸ full (' + (entry.text ?? '').length + ' chars)'}
          </button>
        {/if}
      </div>
    {:else if entry.source}
      <div class="card2">
        <div class="hd sub"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>sub-agent{sourceLabel ? ` · ${sourceLabel}` : ''}</div>
        {#if msg && msg.prose}
          <div class="txt2 dim">{msg.prose}</div>
        {/if}
        {#if msg && msg.kv.length}
          <div class="kvblock">{@render kvRows(msg.kv)}</div>
        {/if}
        {#if !msg}
          <div class="txt2 dim">{entry.text}</div>
        {/if}
      </div>
    {:else if parentLabel}
      <div class="card2">
        <div class="hd par"><svg class="ic" width="13" height="13"><use href="#i-user"/></svg>parent{parentLabel ? ` · ${parentLabel}` : ''}</div>
        {#if msg && msg.prose}
          <div class="txt2 dim">{msg.prose}</div>
        {/if}
        {#if msg && msg.kv.length}
          <div class="kvblock">{@render kvRows(msg.kv)}</div>
        {/if}
        {#if !msg}
          <div class="txt2 dim">{entry.text}</div>
        {/if}
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
        onclick={() => setOpen('tool')}
        onkeydown={onKey(() => setOpen('tool'))}
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
          {@render kvRows(toolKv)}
          {#if toolOut}
            <div class="out"><pre>{outputOpen ? toolOut : preview}</pre></div>
            {#if outputLong}
              <button class="expando" onclick={() => setOpen('output')}>
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
        onclick={() => setOpen('think')}
        onkeydown={onKey(() => setOpen('think'))}
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
    {:else if entry.kind === 'subagent' && subState}
      <div class="stline"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>{subState}</div>
    {:else if entry.kind === 'subagent' && parentLabel}
      <div class="card2">
        <div class="hd sys"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>{subLabel}</div>
        {@render kvRows(subKv)}
      </div>
    {:else if entry.kind === 'subagent'}
      <div class="card2">
        <div class="hd sub"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>sub-agent</div>
        {@render kvRows(subKv)}
      </div>
    {:else if entry.kind === 'spawn-snapshot'}
      <div class="card2">
        <div class="hd sub"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>spawn snapshot</div>
        <div class="txt2">{@html md}</div>
      </div>
    {:else if entry.kind === 'system' && (entry.text ?? '').startsWith('model: ')}
      <div class="stline"><svg class="ic" width="13" height="13"><use href="#i-bot"/></svg>{entry.text}</div>
    {:else if entry.kind === 'system'}
      <div class="card2">
        <div class="hd sys"><svg class="ic" width="13" height="13"><use href="#i-term"/></svg>system</div>
        <div class="txt2 dim">{@html md}</div>
      </div>
    {:else if entry.kind === 'task'}
      <div class="card2" class:collapsed={!taskOpen}>
        <div
          class="hd sys"
          role="button"
          tabindex="0"
          aria-expanded={taskOpen}
          onclick={() => setOpen('task')}
          onkeydown={onKey(() => setOpen('task'))}
        >
          <svg class="ic" width="13" height="13"><use href="#i-check"/></svg>{taskLabel}
          <span class="caret">{taskOpen ? '▾' : '▸'}</span>
        </div>
        {#if taskOpen}
          {@render kvRows(taskKv)}
        {/if}
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
