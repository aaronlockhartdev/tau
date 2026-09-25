<script lang="ts">
  // Workspace tags on top (spec §9): one tag per open workspace with a
  // visible × on every tag, and the
  // focus-mode toggle that hides both side panes (spec: chat is always
  // central; focus mode is a deliberate collapse, not a different app).
  // B2: multiselect on the strip (cmd/ctrl toggle, shift range,
  // click-away clear) with a bulk archive on the selection — the session
  // tree's pattern, mirrored onto the tabs.

  import {
    store,
    openWorkspace,
    closeWorkspace,
    closeWorkspaces,
    tabSelect
  } from '../lib/store.svelte';
  import type { Workspace } from '../lib/protocol';
  import { tick } from 'svelte';

  // The right-click menu: its position plus the selection it acts on.
  let ctx = $state<{ x: number; y: number; ids: string[] } | null>(null);
  // The rendered position: the raw click point, clamped inside the
  // viewport after the first render.
  let ctxPos = $state({ x: 0, y: 0 });
  let ctxMenu: HTMLDivElement | undefined = $state(undefined);

  // Any click inside the strip is the tabs' own (tabSelect), so the
  // document click-away below must not race it — same for the menu.
  function onTabClick(e: MouseEvent, w: Workspace): void {
    ctx = null;
    tabSelect(w.id, e);
    if (!(e.metaKey || e.ctrlKey || e.shiftKey)) void openWorkspace(w);
  }

  // The right-click menu acts on the selection: right-clicking a selected
  // tab offers archive for the whole selection; a single tab reads as its
  // own one-element selection (the session tree's rule).
  function onTabContext(e: MouseEvent, w: Workspace): void {
    e.preventDefault();
    ctx = {
      x: e.clientX,
      y: e.clientY,
      ids: store.tabSelected.includes(w.id) ? store.tabSelected : [w.id]
    };
    ctxPos = { x: e.clientX, y: e.clientY };
  }

  const menuLabel = $derived(ctx && ctx.ids.length > 1 ? `archive (${ctx.ids.length})` : 'archive');

  function applyMenuAction(): void {
    const ids = ctx?.ids ?? [];
    ctx = null;
    void closeWorkspaces(store.workspaces.filter((w) => ids.includes(w.id)));
  }

  $effect(() => {
    if (!ctx && store.tabSelected.length === 0) return;
    const clear = () => {
      store.tabSelected = [];
      store.tabSelAnchor = null;
      ctx = null;
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') clear();
    };
    const close = (e: MouseEvent) => {
      const t = e.target as Element | null;
      if (t?.closest?.('.tabs') || t?.closest?.('.ctxmenu')) return;
      clear();
    };
    document.addEventListener('click', close);
    document.addEventListener('keydown', onKey);
    // The menu is position:fixed at the raw clientX/clientY: a right-click
    // near the bottom/right edge would render off-screen, so after it
    // renders, measure it and shift it inside the window bounds.
    void tick().then(() => {
      const el = ctxMenu;
      if (!ctx || !el) return;
      const r = el.getBoundingClientRect();
      ctxPos = {
        x: Math.max(0, Math.min(ctx.x, window.innerWidth - r.width - 4)),
        y: Math.max(0, Math.min(ctx.y, window.innerHeight - r.height - 4))
      };
    });
    return () => {
      document.removeEventListener('click', close);
      document.removeEventListener('keydown', onKey);
    };
  });
</script>

<div class="bar">
  <span class="logo">τ</span>
  <div class="tabs">
    {#each store.workspaces as w (w.id)}
      <div
        class="tab"
        class:active={store.current && store.sessions[store.current]?.meta.workspace === w.id}
        class:multi={store.tabSelected.includes(w.id)}
      >
        <button class="tabname" oncontextmenu={(e) => onTabContext(e, w)} onclick={(e) => onTabClick(e, w)}>
          <span class="dot"></span>{w.name}
        </button>
        <button class="x" aria-label="Close workspace" onclick={() => closeWorkspace(w)}>×</button>
      </div>
    {/each}
  </div>
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

{#if ctx}
  <div class="ctxmenu" role="menu" bind:this={ctxMenu} style:left="{ctxPos.x}px" style:top="{ctxPos.y}px">
    <button type="button" role="menuitem" onclick={() => applyMenuAction()}>
      {menuLabel}
    </button>
  </div>
{/if}

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
  .tabs {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .tab {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 4px 6px 4px 10px;
    border-radius: 8px;
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
    background: color-mix(in srgb, var(--acc) 10%, transparent);
    border-color: color-mix(in srgb, var(--acc) 28%, transparent);
  }
  .tab.multi {
    color: var(--tx);
    background: color-mix(in srgb, var(--acc) 10%, transparent);
    border-color: color-mix(in srgb, var(--acc) 28%, transparent);
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
    background: var(--faint);
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
    background: color-mix(in srgb, var(--red) 12%, transparent);
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
    background: color-mix(in srgb, var(--acc) 12%, transparent);
  }
  .ctxmenu {
    position: fixed;
    z-index: 20;
    background: var(--panel2);
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 25%);
    padding: 3px;
  }
  .ctxmenu button {
    display: block;
    width: 100%;
    padding: 6px 12px;
    border: none;
    border-radius: 4px;
    background: none;
    color: var(--tx);
    font: 12px var(--mono);
    text-align: left;
    cursor: pointer;
  }
  .ctxmenu button:hover,
  .ctxmenu button:focus-visible {
    background: color-mix(in srgb, var(--acc) 12%, transparent);
  }
</style>
