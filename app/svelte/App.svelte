<script lang="ts">
  // The shell (spec §9): workspace tabs on top; body grid with the chat
  // ALWAYS central — focus mode collapses both side panes (the prototype's
  // grid fix: the center keeps its explicit track when the panes hide).
  // The side panes are placeholder regions: their content is ticket #26.

  import WorkspaceTabs from './components/WorkspaceTabs.svelte';
  import Transcript from './components/Transcript.svelte';
  import Queue from './components/Queue.svelte';
  import Composer from './components/Composer.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import { store } from './lib/store.svelte';

  const cur = $derived(store.current ? store.sessions[store.current] : null);
  const loading = $derived(store.loading);
  const error = $derived(store.error);
  const focus = $derived(store.focus);
  const title = $derived(cur?.meta.title ?? cur?.meta.id ?? 'new session');
  const model = $derived(cur?.meta.model ?? '');
</script>

<div class="shell">
  <WorkspaceTabs />
  <div class="body" class:focus={focus}>
    <aside class="left">
      <div class="pane-tab">files</div>
      <div class="pane-tab">sessions</div>
      <div class="pane-ph">side panes land in ticket #26</div>
    </aside>
    <main class="center">
      <div class="chead">
        <span class="n">{title}</span>
        {#if cur?.turn === 'running'}
          <span class="badge running"><span class="dot"></span>running</span>
        {/if}
        <span class="m">{model}</span>
      </div>
      {#if error}
        <div class="err">⚠ {error}</div>
      {/if}
      {#if loading}
        <div class="empty"><span class="big">loading…</span></div>
      {:else if cur}
        <Transcript />
        <Queue />
        <Composer />
      {:else}
        <div class="empty">
          <span class="big">no workspace open</span>
          <span>use the + tab to open one, or run with ?demo=1 for the 10k fixture</span>
        </div>
      {/if}
    </main>
    <aside class="right">
      <div class="pane-tab">tasks</div>
      <div class="pane-tab">sub-agents</div>
      <div class="pane-ph">side panes land in ticket #26</div>
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
  .pane-tab {
    flex: 1;
    text-align: center;
    padding: 8px;
    font: 11px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--dim);
    border-bottom: 1px solid var(--line);
  }
  .pane-ph {
    padding: 16px;
    font: 11px var(--mono);
    color: #4d5462;
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
    color: #4d5462;
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
    border-color: rgba(76, 194, 255, 0.4);
  }
  .badge.running .dot {
    background: var(--acc);
    animation: pulse 1.2s infinite;
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
    background: rgba(239, 106, 106, 0.08);
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
