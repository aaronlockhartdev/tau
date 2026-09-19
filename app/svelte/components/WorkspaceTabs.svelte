<script lang="ts">
  // Workspace tabs on top (spec §9): one tab per open workspace, the
  // active one closed by its ×, a + to open a new workspace, and the
  // focus-mode toggle that hides both side panes (spec: chat is always
  // central; focus mode is a deliberate collapse, not a different app).

  import { store, openWorkspace, closeWorkspace, addWorkspace } from '../lib/store.svelte';

  let menuOpen = $state(false);
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
      {#if store.current && store.sessions[store.current]?.meta.workspace === w.id}
        <button class="x" aria-label="close workspace" onclick={() => closeWorkspace(w)}>×</button>
      {/if}
    </div>
  {/each}
  <div class="addwrap">
    <button class="add" title="open a workspace" onclick={() => (menuOpen = !menuOpen)}>+</button>
    {#if menuOpen}
      <div class="menu">
        <button class="mi" onclick={() => { menuOpen = false; void addWorkspace(); }}>
          {store.demo ? 'new window' : 'open folder…'}
        </button>
      </div>
    {/if}
  </div>
  <span class="spacer"></span>
  <button
    class="focus"
    class:on={store.focus}
    title="focus mode: hide side panes"
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
    gap: 6px;
    padding: 5px 12px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--dim);
    border: 1px solid transparent;
  }
  .tab:hover {
    color: var(--tx);
    background: var(--panel2);
  }
  .tab.active {
    color: var(--tx);
    background: var(--panel2);
    border-color: var(--line);
  }
  .tab .tabname {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .tab .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #3a4150;
  }
  .tab.active .dot {
    background: var(--green);
  }
  .x {
    margin-left: 4px;
    padding: 0 3px;
    color: var(--dim);
    font-size: 13px;
  }
  .x:hover {
    color: var(--red);
  }
  .addwrap {
    position: relative;
  }
  .add {
    width: 26px;
    height: 26px;
    border-radius: 6px;
    color: var(--dim);
    font-size: 15px;
  }
  .add:hover {
    color: var(--tx);
    background: var(--panel2);
  }
  .menu {
    position: absolute;
    top: 30px;
    left: 0;
    background: var(--panel2);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px;
    z-index: 10;
    min-width: 140px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  }
  .mi {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 10px;
    border-radius: 4px;
    font-size: 12.5px;
    color: var(--tx);
  }
  .mi:hover {
    background: rgba(76, 194, 255, 0.1);
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
