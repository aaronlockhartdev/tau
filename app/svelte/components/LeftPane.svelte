<script lang="ts">
  // Left pane (spec §9, the #11 verdict): tabbed files | sessions. The
  // sessions tab is the MRU-sorted session tree — sub-agent sessions
  // grouped under their parent (collapsible; the active session's chain
  // expanded by default), an archive folder at the bottom, and the badge
  // rule: top-level rows are badged `running` only while the model is
  // generating or a sub-agent is running (inactive = untagged); sub-agent
  // rows keep their full lifecycle tags.
  import { store, pane, ensurePane, newSession, retryDirFetch, currentSession, fileCache, type PaneState } from '../lib/store.svelte';
  import { onActivate } from '../lib/ui';
  import SessionNode from './SessionNode.svelte';
  import { groupIsOpen } from '../lib/sessions';
  import FileNode from './FileNode.svelte';

  const ws = $derived(currentSession()?.meta.workspace ?? null);
  const p = $derived<PaneState | null>(pane(ws));
  $effect(() => {
    if (ws) ensurePane(ws);
  });

  const sessions = $derived.by(() => {
    if (ws === null) return [];
    return Object.values(store.sessions).filter((s) => s.meta.workspace === ws);
  });
  const top = $derived(sessions.filter((s) => !s.parent && !s.archived).sort((a, b) => b.mru - a.mru));
  // Roots only: a child's archive flag is its parent's cascade (the core
  // archives the children with the parent), so listing children here would
  // show them twice — once top-level, once under their parent.
  const archived = $derived(sessions.filter((s) => s.archived && !s.parent).sort((a, b) => b.mru - a.mru));

  // The visible row order of the sessions tab (live list, then the
  // archive folder's open groups) — the flat sequence a shift range
  // spans.
  const visibleIds = $derived.by(() => {
    if (ws === null) return [];
    const active = currentSession();
    const q = pane(ws);
    const out: string[] = [];
    const walk = (ids: string[]): void => {
      for (const id of ids) {
        out.push(id);
        const s = store.sessions[id]!;
        const kids = sessions
          .filter((k) => k.parent === id)
          .sort((a, b) => b.mru - a.mru)
          .map((k) => k.meta.id);
        if (q && kids.length && groupIsOpen(q, s, active, sessions)) walk(kids);
      }
    };
    walk(top.map((s) => s.meta.id));
    if (q?.archOpen) walk(archived.map((s) => s.meta.id));
    return out;
  });

  // Click-away: a click that lands outside every session row clears the
  // selection (document-level, so no static element takes a handler).
  $effect(() => {
    if (ws === null || p?.ltab !== 'sessions') return;
    function onDocClick(e: MouseEvent): void {
      const q = pane(ws);
      if (!q) return;
      if (e.target instanceof Element && !e.target.closest('.trow')) q.selected = [];
    }
    document.addEventListener('click', onDocClick);
    return () => document.removeEventListener('click', onDocClick);
  });

  function setLtab(t: PaneState['ltab']): void {
    const q = pane(ws);
    if (q) q.ltab = t;
  }
</script>

<div class="pane">
  <div class="tabs" role="tablist">
    <button class="tab" class:on={p?.ltab === 'files'} disabled={!p} onclick={() => { if (p) setLtab('files'); }}>files</button>
    <button class="tab" class:on={p?.ltab === 'sessions'} disabled={!p} onclick={() => { if (p) setLtab('sessions'); }}>sessions</button>
  </div>
  {#if p && ws}
    {#if p.ltab === 'files'}
      {@const root = fileCache(ws)}
      <div class="sec">
        {#if root?.['.']}
          <div class="tree">
            {#each root['.'] as f (f.path)}
              <FileNode entry={f} ws={ws} />
            {/each}
          </div>
        {:else if store.fileErrors[ws]?.['.']}
          <div
            class="note fail"
            role="button"
            tabindex="0"
            onclick={() => retryDirFetch(ws, '.')}
            onkeydown={onActivate(() => retryDirFetch(ws, '.'))}
          >failed to list — click to retry</div>
        {:else}
          <div class="note">No files listed yet.</div>
        {/if}
      </div>
    {:else}
      <div class="sec">
        <div class="tree">
          <button class="new" type="button" title="new session — ⌘N" onclick={() => void newSession(ws)}>
            <svg class="ci" width="10" height="10"><use href="#i-plus" /></svg> new session
          </button>
          {#if top.length === 0}
            <div class="empty"><span class="big">No sessions yet</span></div>
          {:else}
            {#each top as s (s.meta.id)}
              <SessionNode session={s} ws={ws} sessions={sessions} visibleIds={visibleIds} />
            {/each}
          {/if}
        </div>
        {#if archived.length}
        <div class="arch">
          <button class="arch-h" type="button" onclick={() => { const q = pane(ws); if (q) q.archOpen = !q.archOpen; }}>
            <svg class="ci" width="10" height="10" style:transform={p.archOpen ? 'rotate(0deg)' : 'rotate(-90deg)'}><use href="#i-chev" /></svg> archive · {archived.length}
          </button>
          {#if p.archOpen}
            {#each archived as s (s.meta.id)}
              <SessionNode session={s} depth={1} ws={ws} sessions={sessions} visibleIds={visibleIds} />
            {/each}
          {/if}
        </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 0 12px 8px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .tab {
    padding: 3px 9px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--faint);
    border-radius: 6px;
  }
  .tab:hover {
    color: var(--dim);
  }
  .tab.on {
    color: var(--tx);
    background: var(--raised);
  }
  .sec {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }
  .note {
    padding: 14px;
    font: 11px var(--mono);
    color: var(--faint);
    line-height: 1.6;
  }
  .note.fail {
    color: var(--red);
    cursor: pointer;
  }
  .tree {
    padding-top: 4px;
  }
  .new {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: auto;
    margin: 8px 8px 4px auto;
    padding: 2px 8px;
    font-size: 11px;
    font-weight: 600;
    line-height: 1.4;
    color: var(--dim);
    background: none;
    border: 1px solid var(--line2);
    border-radius: 6px;
    cursor: pointer;
  }
  .new:hover {
    color: var(--tx);
    border-color: #3a3d47;
  }
  .new .ci {
    display: block;
    flex: none;
  }
  .arch {
    border-top: 1px solid var(--line);
    margin-top: 8px;
    flex: none;
  }
  .arch-h {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    text-align: left;
    padding: 10px 8px 5px;
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--faint);
  }
  .arch-h .ci {
    display: block;
    transition: transform 0.12s;
  }
  .empty {
    padding: 24px;
    text-align: center;
    font: 11px var(--mono);
    color: var(--faint);
  }
</style>
