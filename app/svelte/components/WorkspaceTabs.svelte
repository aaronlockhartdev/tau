<script lang="ts">
  // Workspace tags on top (spec §9): one tag per open workspace with a
  // visible × on every tag, and the
  // focus-mode toggle that hides both side panes (spec: chat is always
  // central; focus mode is a deliberate collapse, not a different app).

  import { store, openWorkspace, closeWorkspace } from '../lib/store.svelte';
</script>

<div class="bar">
  <span class="logo">τ</span>
  {#each store.workspaces as w (w.id)}
    <div
      class="tab"
      class:active={store.current && store.sessions[store.current]?.meta.workspace === w.id}
    >
      <button class="tabname" onclick={() => openWorkspace(w)}>
        <span class="dot"></span>{w.name}
      </button>
      <button class="x" aria-label="Close workspace" onclick={() => closeWorkspace(w)}>×</button>
    </div>
  {/each}
  <span class="spacer"></span>
  <button
    class="focus"
    class:on={store.focus}
    title="Focus mode: hide side panes"
    onclick={() => (store.focus = !store.focus)}
  >
    ⤢
  </button>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 38px;
    padding: 0 10px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
    user-select: none;
  }
  .logo {
    font-size: 18px;
    font-weight: 700;
    color: var(--acc);
    padding: 0 8px 0 2px;
  }
  .tab {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 4px 6px 4px 10px;
    border-radius: 6px;
    font-size: 12px;
    color: var(--dim);
    border: 1px solid transparent;
  }
  .tab:hover {
    color: var(--tx);
    background: var(--panel2);
  }
  .tab.active {
    color: var(--tx);
    background: rgba(76, 194, 255, 0.10);
    border-color: rgba(76, 194, 255, 0.28);
  }
  .tab .tabname {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .tab .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #3a4150;
  }
  .tab.active .dot {
    background: var(--green);
  }
  .x {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    color: var(--dim);
    font-size: 12px;
    line-height: 1;
  }
  .x:hover {
    color: var(--red);
    background: rgba(255, 90, 90, 0.12);
  }
  .spacer {
    flex: 1;
  }
  .focus {
    width: 28px;
    height: 28px;
    border-radius: 6px;
    color: var(--dim);
    font-size: 14px;
  }
  .focus:hover {
    color: var(--tx);
    background: var(--panel2);
  }
  .focus.on {
    color: var(--acc);
    background: rgba(76, 194, 255, 0.12);
  }
</style>
