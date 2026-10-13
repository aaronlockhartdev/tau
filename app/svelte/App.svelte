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
  import ModelMenu from './components/ModelMenu.svelte';
  import PeekSheet from './components/PeekSheet.svelte';
  import AttentionSheet from './components/AttentionSheet.svelte';
  import CommandPalette from './components/CommandPalette.svelte';
  import FleetList from './components/FleetList.svelte';
  import FocusRails from './components/FocusRails.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { store, toggleAllReasoning, newSession, windowTitle, currentSession, setPeek, pane, switchSession } from './lib/store.svelte';
  import { isTauri } from './lib/protocol';
  // The dynamic window title (the center header was deleted): the session's
  // name lives in the title bar and the status bar, not a header row.
  $effect(() => {
    const title = windowTitle();
    document.title = title;
    try {
      if (isTauri()) void getCurrentWindow().setTitle(title).catch(() => {});
    } catch {
      // No Tauri window (demo/dev): document.title is the whole job.
    }
  });
  // 'r' toggles every reasoning line at once (V2 thinking lines); skipped
  // while a field has focus so it never fights the composer. ⌘/Ctrl+N opens a
  // new session in the active workspace (app-global: it fires from the
  // composer too, where it is the natural "start over" key).
  // The keymap (workbench P6): ⌘K palette, ⌘\ fleet, ⌘. attention, f focus
  // mode, j/k session nav, Enter opens the selected session. The Escape
  // cascade closes overlays in the order they open: palette → attention →
  // fleet → peek → inspector → follow. Single letters are skipped while a
  // text field has focus so they never fight the composer.
  function typing(e: KeyboardEvent): boolean {
    const t = e.target as HTMLElement | null;
    return !!(t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable));
  }
  function visibleSessions(): string[] {
    const ws = currentSession()?.meta.workspace ?? null;
    if (!ws) return [];
    return Object.values(store.sessions)
      .filter((s) => s.meta.workspace === ws && !s.parent && !s.archived)
      .sort((a, b) => b.mru - a.mru)
      .map((s) => s.meta.id);
  }
  function navSession(delta: number): void {
    const ws = currentSession()?.meta.workspace ?? null;
    const q = ws ? pane(ws) : null;
    if (!q || q.ltab !== 'sessions') return;
    const ids = visibleSessions();
    if (ids.length === 0) return;
    const anchor = q.selAnchor ? ids.indexOf(q.selAnchor) : -1;
    const next = (anchor + delta + ids.length) % ids.length;
    q.selected = [ids[next]!];
    q.selAnchor = ids[next]!;
  }
  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      if (store.paletteOpen) {
        store.paletteOpen = false;
        return;
      }
      if (store.attentionOpen) {
        store.attentionOpen = false;
        return;
      }
      if (store.fleetOpen) {
        store.fleetOpen = false;
        return;
      }
      if (store.peek) {
        setPeek(null);
        return;
      }
      if (store.focusInspector) {
        store.focusInspector = null;
        return;
      }
      if (store.follow) {
        store.follow = null;
        return;
      }
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      store.paletteOpen = !store.paletteOpen;
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.key === '\\') {
      e.preventDefault();
      store.fleetOpen = !store.fleetOpen;
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.key === '.') {
      e.preventDefault();
      store.attentionOpen = !store.attentionOpen;
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'n') {
      e.preventDefault();
      const ws = currentSession()?.meta.workspace ?? null;
      if (ws) void newSession(ws);
      return;
    }
    if (typing(e)) return;
    const k = e.key.toLowerCase();
    if (k === 'j') navSession(1);
    else if (k === 'k') navSession(-1);
    else if (k === 'f') store.focus = !store.focus;
    else if (k === 'r') toggleAllReasoning();
    else if (e.key === 'Enter') {
      const ws = currentSession()?.meta.workspace ?? null;
      const q = ws ? pane(ws) : null;
      const sel = q?.selAnchor;
      if (q && sel) void switchSession(sel);
    }
  }
  onMount(() => window.addEventListener('keydown', onKeydown));
  onDestroy(() => window.removeEventListener('keydown', onKeydown));
  const cur = $derived(currentSession());
  const loading = $derived(store.loading);
  const error = $derived(store.error);
  const focus = $derived(store.focus);
</script>

<div class="shell">
  <svg class="sprites" width="0" height="0" aria-hidden="true">
    <symbol id="i-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></symbol>
    <symbol id="i-spark" viewBox="0 0 24 24" fill="currentColor"><path d="M12 3l1.9 5.8 5.8 1.9-5.8 1.9L12 18.4l-1.9-5.8L4.3 10.7l5.8-1.9z"/></symbol>
    <symbol id="i-bot" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="8" width="14" height="10" rx="2"/><path d="M12 8V4M9 13h.01M15 13h.01"/></symbol>
    <symbol id="i-book" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 4h11a3 3 0 013 3v13H8a3 3 0 01-3-3z"/><path d="M5 4v13"/></symbol>
    <symbol id="i-term" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 7l4 4-4 4M11 15h6"/></symbol>
    <symbol id="i-file" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/></symbol>
    <symbol id="i-plus" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M5 12h14"/></symbol>
    <symbol id="i-search" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="10.5" cy="10.5" r="5.5"/><path d="M15 15l5 5"/></symbol>
    <symbol id="i-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l5 5 9-9"/></symbol>
    <symbol id="i-user" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"/><path d="M5 20a7 7 0 0114 0"/></symbol>
    <symbol id="i-archive" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11a1 1 0 001 1h12a1 1 0 001-1V8M10 12h4"/></symbol>
    <symbol id="i-bell" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9a6 6 0 0112 0c0 5 2 6 2 6H4s2-1 2-6"/><path d="M10 19a2 2 0 004 0"/></symbol>
    <symbol id="i-focus" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"/></symbol>
    <symbol id="i-send" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5 12l7-7 7 7"/></symbol>
    <symbol id="i-stop" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></symbol>
    <symbol id="i-bolt" viewBox="0 0 24 24" fill="currentColor"><path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"/></symbol>
    <symbol id="i-blocked" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M5.5 5.5l13 13"/></symbol>
    <symbol id="i-alert" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3L1.5 21h21L12 3z"/><path d="M12 10v5M12 18h.01"/></symbol>
  </svg>
  <WorkspaceTabs />
  <div class="body" class:focus={focus}>
    <aside class="left">
      <LeftPane />
    </aside>
    <main class="center">
      {#if error}
        <div class="err"><svg class="ei" viewBox="0 0 24 24" aria-hidden="true"><use href="#i-alert" /></svg>{error}</div>
      {/if}
      {#if loading}
        <div class="empty"><span class="big">Loading…</span></div>
      {:else if cur}
        <div class="readcol">
          {#key store.current}
            <Transcript />
          {/key}
          <Queue />
          <Composer />
        </div>
        {#if store.modelMenuOpen}
          <ModelMenu />
        {/if}
        <PeekSheet />
        <AttentionSheet />
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
    <FocusRails />
  </div>
  <StatusBar />
  <CommandPalette />
  <FleetList />
</div>

<style>
  .shell {
    display: grid;
    grid-template-rows: auto 1fr auto;
    height: 100vh;
  }
  .body {
    position: relative;
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
  .readcol {
    display: contents;
  }
  /* Focus mode is a reading view: the transcript column is width-
     capped and centred so the panel background shows on both sides. */
  .body.focus .readcol {
    display: flex;
    flex-direction: column;
    min-height: 0;
    width: 100%;
    max-width: 800px;
    margin: 0 auto;
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
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .err {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 16px;
    font: 11.5px var(--mono);
    color: var(--red);
    background: color-mix(in srgb, var(--red) 8%, transparent);
    border-bottom: 1px solid var(--line);
  }
  .err .ei {
    width: 13px;
    height: 13px;
    flex: none;
    stroke: currentColor;
    fill: none;
    stroke-width: 2;
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
