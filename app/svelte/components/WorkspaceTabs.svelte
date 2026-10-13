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
    attention,
    currentSession,
    openWorkspace,
    closeWorkspace,
    closeWorkspaces,
    tabSelect
  } from '../lib/store.svelte';
  import type { Workspace } from '../lib/protocol';
  import { tick } from 'svelte';
  import { ctxClamp } from '../lib/ui';
  import { Bell, Maximize2 } from '@lucide/svelte';

  // The right-click menu: its position plus the selection it acts on.
  let ctx = $state<{ x: number; y: number; ids: string[] } | null>(null);
  // The rendered position (clamped into the viewport after first render)
  // plus the menu element the clamp measures.
  const menu = $state<{ pos: { x: number; y: number }; el: HTMLDivElement | undefined }>({
    pos: { x: 0, y: 0 },
    el: undefined
  });

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
    menu.pos = { x: e.clientX, y: e.clientY };
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
    void tick().then(() => {
      const el = menu.el;
      if (!ctx || !el) return;
      menu.pos = ctxClamp(ctx, el);
    });
    return () => {
      document.removeEventListener('click', close);
      document.removeEventListener('keydown', onKey);
    };
  });
</script>

<div class="bar" data-tauri-drag-region>
  <span class="logo">τ</span>
  <div class="tabs">
    {#each store.workspaces as w (w.id)}
      <div
        class="tab"
        class:active={currentSession()?.meta.workspace === w.id}
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
    <Maximize2 class="fi" size={15} aria-hidden="true" />
  </button>
  <button
    class="bell"
    class:on={store.attentionOpen}
    title="Needs your attention"
    onclick={() => (store.attentionOpen = !store.attentionOpen)}
  >
    <Bell class="bi" size={15} aria-hidden="true" />
    {#if attention().length > 0}
      <span class="count">{attention().length}</span>
    {/if}
  </button>
</div>

{#if ctx}
  <div class="ctxmenu" role="menu" bind:this={menu.el} style:left="{menu.pos.x}px" style:top="{menu.pos.y}px">
    <button type="button" role="menuitem" onclick={() => applyMenuAction()}>
      {menuLabel}
    </button>
  </div>
{/if}

<style>
  .bar {
    display: flex;
    align-items: stretch;
    gap: 1px;
    height: 48px;
    padding: 0 12px 0 96px;
    background: var(--bg);
    border-bottom: 1px solid var(--line);
    user-select: none;
  }
  .logo {
    align-self: center;
    font-size: 14px;
    font-weight: 700;
    color: var(--tx);
    padding: 0 10px 0 2px;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .tab {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    /* A floating inset tab: a 30px pill centred in the 48px bar, all
       corners rounded, a hairline border on the active one. The old
       full-height slab with notched bottom corners read as a block, not
       a tab. */
    height: 30px;
    padding: 0 8px 0 11px;
    border-radius: 8px;
    font-size: 12.5px;
    font-weight: 500;
    color: var(--dim);
    border: 1px solid transparent;
  }
  .tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--tx);
  }
  /* The active tab merges into the pane below it: panel fill plus the two
     radial-gradient notches where its top corners meet the bar. */
  .tab.active,
  .tab.multi {
    color: var(--tx);
    font-weight: 600;
    background: var(--panel);
    border-color: var(--line);
  }
  .tab .tabname {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .tab .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--faint);
  }
  .tab.active .dot {
    background: var(--green);
    animation: pulse 1.4s infinite;
  }
  .x {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    color: var(--faint);
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
    align-self: center;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    color: var(--dim);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .focus:hover {
    color: var(--tx);
    background: rgba(255, 255, 255, 0.07);
  }
  .focus.on {
    color: var(--acc);
    background: color-mix(in srgb, var(--acc) 12%, transparent);
  }
  .bell {
    position: relative;
    align-self: center;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    background: none;
    border: none;
    color: var(--dim);
    color: var(--dim);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .focus :global(.fi),
  .bell :global(.bi) {
    flex: none;
  }
  .bell:hover {
    color: var(--tx);
    background: rgba(255, 255, 255, 0.07);
  }
  .bell.on {
    color: var(--red);
  }
  .bell .count {
    position: absolute;
    top: -2px;
    right: -2px;
    min-width: 14px;
    padding: 0 3px;
    border-radius: 7px;
    background: var(--red);
    color: #fff;
    font: 700 9px/14px var(--mono);
    text-align: center;
  }
</style>
