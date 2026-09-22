<script lang="ts">
  // The shell (spec §9): workspace tabs on top; body grid with the chat
  // ALWAYS central — focus mode collapses both side panes (the prototype's
  // grid fix: the center keeps its explicit track when the panes hide).
  // The side panes are ticket #26: left = files | sessions tree, right =
  // tasks | sub-agents (both tabbed, per the #11 verdict).

  import WorkspaceTabs from './components/WorkspaceTabs.svelte';
  import Transcript from './components/Transcript.svelte';
  import LeftPane from './components/LeftPane.svelte';
  import RightPane from './components/RightPane.svelte';
  import Queue from './components/Queue.svelte';
  import Composer from './components/Composer.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { store, toggleAllReasoning } from './lib/store.svelte';
  // 'r' toggles every reasoning line at once (V2 thinking lines); skipped
  // while a field has focus so it never fights the composer.
  function onKeydown(e: KeyboardEvent): void {
    if (e.key !== 'r' || e.metaKey || e.ctrlKey || e.altKey) return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
    toggleAllReasoning();
  }
  onMount(() => window.addEventListener('keydown', onKeydown));
  onDestroy(() => window.removeEventListener('keydown', onKeydown));
  const cur = $derived(store.current ? store.sessions[store.current] : null);
  const loading = $derived(store.loading);
  const error = $derived(store.error);
  const focus = $derived(store.focus);
  const title = $derived(cur?.meta.title ?? cur?.meta.id ?? 'New session');
  const model = $derived(cur?.meta.model ?? '');
</script>

<div class="shell">
  <svg class="sprites" width="0" height="0" aria-hidden="true">
    <symbol id="i-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></symbol>
    <symbol id="i-spark" viewBox="0 0 24 24" fill="currentColor"><path d="M12 3l1.9 5.8 5.8 1.9-5.8 1.9L12 18.4l-1.9-5.8L4.3 10.7l5.8-1.9z"/></symbol>
    <symbol id="i-bot" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="8" width="14" height="10" rx="2"/><path d="M12 8V4M9 13h.01M15 13h.01"/></symbol>
    <symbol id="i-book" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 4h11a3 3 0 013 3v13H8a3 3 0 01-3-3z"/><path d="M5 4v13"/></symbol>
    <symbol id="i-term" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 7l4 4-4 4M11 15h6"/></symbol>
    <symbol id="i-file" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/></symbol>
    <symbol id="i-search" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="10.5" cy="10.5" r="5.5"/><path d="M15 15l5 5"/></symbol>
    <symbol id="i-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l5 5 9-9"/></symbol>
    <symbol id="i-user" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"/><path d="M5 20a7 7 0 0114 0"/></symbol>
  </svg>
  <WorkspaceTabs />
  <div class="body" class:focus={focus}>
    <aside class="left">
      <LeftPane />
    </aside>
    <main class="center">
      <div class="chead">
        <span class="n">{title}</span>
        <!-- #11 verdict, TOP-LEVEL only: badged `running` while this session
             is generating or one of its sub-agents is running. A child
             session's header shows its own lifecycle state instead (its
             sub-agent's state, not its parent's badge rule). -->
        {#if cur && !cur.parent && (cur.turn === 'running' || cur.subagents.some((x) => x.state === 'running'))}
          <span class="badge running"><span class="dot"></span>running</span>
        {:else if cur && cur.parent}
          <span class="badge {cur.state}"><span class="dot"></span>{cur.state}{cur.state === 'idle' && cur.waiting_on ? ` · ${cur.waiting_on}` : ''}</span>
        {/if}
        <span class="m">{model}</span>
      </div>
      {#if error}
        <div class="err">⚠ {error}</div>
      {/if}
      {#if loading}
        <div class="empty"><span class="big">Loading…</span></div>
      {:else if cur}
        {#key store.current}
        <Transcript />
        {/key}
        <Queue />
        <Composer />
      {:else}
        <div class="empty">
          <span class="big">No workspace open</span>
          <span>Open a project with File → Open Folder… (⌘O)</span>
        </div>
      {/if}
    </main>
    <aside class="right">
      <RightPane />
    </aside>
  </div>
  <StatusBar />
</div>

<style>
  .shell {
    display: grid;
    grid-template-rows: auto 1fr auto;
    height: 100vh;
  }
  .body {
    display: grid;
    grid-template-columns: 280px 1fr 330px;
    grid-template-rows: 1fr;
    min-height: 0;
  }
  .body.focus {
    grid-template-columns: 0 1fr 0;
  }
  .body.focus .left,
  .body.focus .right {
    display: none;
  }
  /* The center must stay in its own track when the side panes are hidden. */
  .body.focus .center {
    grid-column: 2;
    grid-row: 1;
  }
  .left {
    background: var(--panel);
    border-right: 1px solid var(--line);
    overflow-y: auto;
    min-height: 0;
  }
  .right {
    background: var(--panel);
    border-left: 1px solid var(--line);
    overflow-y: auto;
    min-height: 0;
  }
  .center {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .chead {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    flex: none;
  }
  .chead .n {
    font-weight: 600;
    font-size: 13.5px;
  }
  .chead .m {
    margin-left: auto;
    font: 10.5px var(--mono);
    color: var(--faint);
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font: 10.5px/1.6 var(--mono);
    padding: 1px 7px;
    border-radius: 9px;
    border: 1px solid var(--line);
    color: var(--dim);
    white-space: nowrap;
  }
  .badge .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--dim);
  }
  .badge.running {
    color: var(--acc);
    border-color: color-mix(in srgb, var(--acc) 40%, transparent);
  }
  .badge.running .dot {
    background: var(--acc);
    animation: pulse 1.2s infinite;
  }
  /* The child header's own state tag (the badge rule is top-level only). */
  .badge.idle {
    color: var(--amber);
    border-color: color-mix(in srgb, var(--amber) 40%, transparent);
  }
  .badge.done {
    color: var(--green);
    border-color: color-mix(in srgb, var(--green) 40%, transparent);
  }
  .badge.failed {
    color: var(--red);
    border-color: color-mix(in srgb, var(--red) 40%, transparent);
  }
  .badge.stopped {
    color: var(--dim);
    border-color: color-mix(in srgb, var(--dim) 40%, transparent);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .err {
    padding: 6px 16px;
    font: 11.5px var(--mono);
    color: var(--red);
    background: color-mix(in srgb, var(--red) 8%, transparent);
    border-bottom: 1px solid var(--line);
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--dim);
  }
  .empty .big {
    font-size: 15px;
    color: var(--tx);
  }
</style>
