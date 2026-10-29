<script lang="ts">
  // The shell (spec §9): workspace tabs on top; body grid with the chat
  // ALWAYS central — focus mode collapses both side panes (the prototype's
  // grid fix: the center keeps its explicit track when the panes hide).
  // The side panes are ticket #26: left = files | sessions tree, right =
  // sub-agents.

  import WorkspaceTabs from './components/WorkspaceTabs.svelte';
  import Transcript from './components/Transcript.svelte';
  import LeftPane from './components/LeftPane.svelte';
  import RightPane from './components/RightPane.svelte';
  import Queue from './components/Queue.svelte';
  import Composer from './components/Composer.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import ModelMenu from './components/ModelMenu.svelte';
  import ChangesView from './components/ChangesView.svelte';
  import AttentionSheet from './components/AttentionSheet.svelte';
  import CommandPalette from './components/CommandPalette.svelte';
  import FleetList from './components/FleetList.svelte';
  import FocusRails from './components/FocusRails.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { store, toggleAllReasoning, newSession, windowTitle, currentSession, setCenterMode, pane, switchSession } from './lib/store/store.svelte';
  import { FileText, TriangleAlert } from '@lucide/svelte';
  import { isTauri } from './lib/wire/protocol';
  // #195: markdown links open externally instead of navigating the webview.
  import { attachExternalLinkHandler } from './lib/display/externalLinks';
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
      .sort((a, b) => b.last_modified - a.last_modified)
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
      if (centerFiles && curWs) {
        setCenterMode(curWs, 'chat');
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
  onDestroy(attachExternalLinkHandler());
  const cur = $derived(currentSession());
  const loading = $derived(store.loading);
  const error = $derived(store.error);
  const focus = $derived(store.focus);
  const curWs = $derived(currentSession()?.meta.workspace ?? null);
  // The center column's view: the file viewer when pane.center is 'files', else
  // the transcript. Open files are separate state, so the transcript shows a
  // "resume" affordance while any are open.
  const centerFiles = $derived(pane(curWs)?.center === 'files');
  const openCount = $derived(curWs ? (store.openFiles[curWs] ?? []).length : 0);
</script>

<div class="shell">
  <WorkspaceTabs />
  <div class="body" class:focus={focus}>
    <aside class="left">
      <LeftPane />
    </aside>
    <main class="center">
      {#if error}
        <div class="err"><TriangleAlert class="ei" size={13} aria-hidden="true" />{error}</div>
      {/if}
      {#if loading}
        <div class="empty"><span class="big">Loading…</span></div>
      {:else if cur}
        <div class="readcol">
          {#key store.current}
            {#if centerFiles}
              <ChangesView />
            {:else}
              {#if openCount > 0}
                <button class="resume" onclick={() => curWs && setCenterMode(curWs, 'files')}>
                  <FileText size={12} aria-hidden="true" />
                  <span>{openCount} file{openCount === 1 ? '' : 's'} open</span>
                </button>
              {/if}
              <Transcript />
            {/if}
          {/key}
          <Queue />
          <Composer />
        </div>
        {#if store.modelMenuOpen}
          <ModelMenu />
        {/if}
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
  /* The transcript's "files are open" affordance: a compact pill that toggles
     the center back to the file viewer (the open set is untouched). */
  .resume {
    display: flex;
    align-items: center;
    gap: 6px;
    align-self: flex-start;
    margin: 6px 16px 0;
    padding: 4px 10px;
    border: 1px solid var(--line2);
    border-radius: 6px;
    color: var(--dim);
    font: 11px var(--mono);
  }
  .resume:hover {
    color: var(--tx);
    border-color: var(--acc);
  }
  .resume :global(svg) {
    flex: none;
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
  .err :global(.ei) {
    flex: none;
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
