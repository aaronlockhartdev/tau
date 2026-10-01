<script lang="ts">
  // The expanded card shells (tool chip + card) out of EntryCard (#40): the
  // expansion/details sub-render with its own scoped styles. Expansion state
  // and its toggles stay in EntryCard (the store map is the persistence
  // layer); this child is a stateless renderer over them.
  import type { ExpandSlot, KvRow, Section, Shell } from '../lib/presentation';

  type CardShellShell = Extract<Shell, { kind: 'tool' | 'card' }>;

  let {
    shell,
    open,
    setOpen,
    onKey
  }: {
    shell: CardShellShell;
    open: Record<ExpandSlot, boolean>;
    setOpen: (slot: ExpandSlot) => void;
    onKey: (fn: () => void) => (e: KeyboardEvent) => void;
  } = $props();
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
{#if shell.kind === 'tool'}
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

<style>
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
  .hd .caret {
    margin-left: auto;
    color: var(--faint);
    font-size: 10px;
  }
  .hd[role='button'] {
    cursor: pointer;
  }
</style>
