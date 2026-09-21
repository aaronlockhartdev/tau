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
  import { store } from './lib/store.svelte';

  const cur = $derived(store.current ? store.sessions[store.current] : null);
  const loading = $derived(store.loading);
  const error = $derived(store.error);
  const focus = $derived(store.focus);
  const title = $derived(cur?.meta.title ?? cur?.meta.id ?? 'New session');
  const model = $derived(cur?.meta.model ?? '');
</script>

<div class="shell">
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
          <span>Open a project with File → Open Folder… (⌘O); workspaces are remembered from previous runs</span>
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
  /* The child header's own state tag (the badge rule is top-level only). */
  .badge.idle {
    color: #e5c07b;
    border-color: rgba(229, 192, 123, 0.4);
  }
  .badge.done {
    color: #7ec97e;
    border-color: rgba(126, 201, 126, 0.4);
  }
  .badge.failed {
    color: var(--red);
    border-color: rgba(240, 109, 109, 0.4);
  }
  .badge.stopped {
    color: #9aa4b5;
    border-color: rgba(154, 164, 181, 0.4);
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
