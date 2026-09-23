<script lang="ts">
  // Left pane (spec §9, the #11 verdict): tabbed files | sessions. The
  // sessions tab is the MRU-sorted session tree — sub-agent sessions
  // grouped under their parent (collapsible; the active session's chain
  // expanded by default), an archive folder at the bottom, and the badge
  // rule: top-level rows are badged `running` only while the model is
  // generating or a sub-agent is running (inactive = untagged); sub-agent
  // rows keep their full lifecycle tags.
  import { store, pane, ensurePane, newSession, type PaneState } from '../lib/store.svelte';
  import SessionNode from './SessionNode.svelte';
  import FileNode from './FileNode.svelte';

  const ws = $derived(store.current ? store.sessions[store.current]?.meta.workspace ?? null : null);
  const p = $derived<PaneState | null>(pane(ws));
  $effect(() => {
    if (ws) ensurePane(ws);
  });

  const sessions = $derived.by(() => {
    if (ws === null) return [];
    return Object.values(store.sessions).filter((s) => s.meta.workspace === ws);
  });
  const top = $derived(sessions.filter((s) => !s.parent && !s.archived).sort((a, b) => b.mru - a.mru));
  const archived = $derived(sessions.filter((s) => s.archived).sort((a, b) => b.mru - a.mru));

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
      <div class="sec">
        {#if ws && store.files[ws]?.['.']}
          <div class="tree">
            {#each store.files[ws]['.'] as f (f.path)}
              <FileNode entry={f} ws={ws} />
            {/each}
          </div>
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
              <SessionNode session={s} ws={ws} sessions={sessions} />
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
              <SessionNode session={s} depth={1} ws={ws} sessions={sessions} />
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
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .tab {
    flex: 1;
    text-align: center;
    padding: 8px;
    font: 11px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--dim);
  }
  .tab.on {
    color: var(--acc);
    box-shadow: inset 0 -2px 0 var(--acc);
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
  .tree {
    padding-top: 4px;
  }
  .new {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    padding: 4.5px 8px;
    font-size: 12.5px;
    line-height: 1;
    color: var(--faint);
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
  }
  .new:hover {
    color: var(--text);
    background: var(--panel2);
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
    padding: 8px 12px 4px;
    font: 10px var(--mono);
    letter-spacing: 0.08em;
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
