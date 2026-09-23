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
  import { onMount, onDestroy } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { store, toggleAllReasoning, newSession, windowTitle } from './lib/store.svelte';
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
  function onKeydown(e: KeyboardEvent): void {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'n') {
      e.preventDefault();
      const ws = store.current ? (store.sessions[store.current]?.meta.workspace ?? null) : null;
      if (ws) void newSession(ws);
      return;
    }
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
  </svg>
  <WorkspaceTabs />
  <div class="body" class:focus={focus}>
    <aside class="left">
      <LeftPane />
    </aside>
    <main class="center">
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
        {#if store.modelMenuOpen}
          <ModelMenu />
        {/if}
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
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
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
