<script lang="ts">
  // Left pane (spec §9, the #11 verdict): tabbed files | sessions. The
  // sessions tab is the MRU-sorted session tree — sub-agent sessions
  // grouped under their parent (collapsible; the active session's group
  // expanded by default), an archive folder at the bottom, and the badge
  // rule: top-level rows are badged `running` only while the model is
  // generating or a sub-agent is running (inactive = untagged); sub-agent
  // rows keep their full lifecycle tags.
  import {
    store,
    pane,
    ensurePane,
    openSessionById,
    renameSession,
    type PaneState
  } from '../lib/store.svelte';
  import { tick } from 'svelte';
  import type { SessionState } from '../lib/store.svelte';

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
  const active = $derived(store.current ? store.sessions[store.current] : null);
  // Inline rename: a double-clicked title becomes an input (Enter/blur
  // commits, Esc cancels).
  let renaming: string | null = $state(null);
  let renameText = $state('');
  let renameInput: HTMLInputElement | undefined = $state(undefined);
  function startRename(s: SessionState): void {
    renaming = s.meta.id;
    renameText = s.meta.title ?? '';
    // Double-click leaves focus on the row; pull it into the input so
    // typing works immediately.
    void tick().then(() => {
      renameInput?.focus();
      renameInput?.select();
    });
  }
  function commitRename(id: string): void {
    if (renaming !== id) return;
    renaming = null;
    void renameSession(id, renameText);
  }

  const onKey = (fn: () => void) => (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      fn();
    }
  };

  function childrenOf(sid: string): SessionState[] {
    return sessions.filter((s) => s.parent === sid).sort((a, b) => b.mru - a.mru);
  }
  function sessionRunning(s: SessionState): boolean {
    return s.turn === 'running' || s.subagents.some((x) => x.state === 'running');
  }
  function childInfo(c: SessionState) {
    return active?.subagents.find((x) => x.child === c.meta.id) ?? null;
  }
  const fmtAgo = (ms: number) => {
    const m = (Date.now() - ms) / 60000;
    return m < 1 ? 'just now' : m < 60 ? `${Math.round(m)}m` : `${Math.round(m / 60)}h`;
  };

  function setLtab(t: PaneState['ltab']): void {
    const q = pane(ws);
    if (q) q.ltab = t;
  }
  function groupOpen(sid: string): boolean {
    const q = pane(ws);
    if (!q) return false;
    if (q.openGroups.includes(sid)) return true;
    if (q.openGroups.length !== 0) return false;
    // Default view: the group containing the active session — the parent
    // itself, or an ancestor while a (nested) child of it is active, so the
    // row the user just opened stays visible.
    let a: SessionState | null = active;
    while (a) {
      if (a.meta.id === sid) return true;
      a = a.parent ? (store.sessions[a.parent] ?? null) : null;
    }
    return false;
  }
  function toggleGroup(sid: string): void {
    const q = pane(ws);
    if (!q) return;
    if (q.openGroups.length === 0) q.openGroups = active ? [active.meta.id] : [];
    q.openGroups = groupOpen(sid) ? q.openGroups.filter((x) => x !== sid) : [...q.openGroups, sid];
  }
</script>

<div class="pane">
  <div class="tabs" role="tablist">
    <button class="tab" class:on={p?.ltab === 'files'} disabled={!p} onclick={() => { if (p) setLtab('files'); }}>files</button>
    <button class="tab" class:on={p?.ltab === 'sessions'} disabled={!p} onclick={() => { if (p) setLtab('sessions'); }}>sessions</button>
  </div>
  {#if p}
    {#if p.ltab === 'files'}
      <div class="sec">
        <div class="note">v0 has no directory-listing command.</div>
      </div>
    {:else}
      <div class="sec">
        <div class="tree">
          {#if top.length === 0}
            <div class="empty"><span class="big">No sessions yet</span></div>
          {:else}
            {#each top as s (s.meta.id)}
              <div class="node">
                <div
                  class="srow"
                  class:sel={store.current === s.meta.id}
                  role="button"
                  tabindex="0"
                  onclick={() => openSessionById(s.meta.id)}
                  onkeydown={onKey(() => openSessionById(s.meta.id))}
                  ondblclick={() => startRename(s)}
                >
                  <button
                    class="chev"
                    type="button"
                    class:has={childrenOf(s.meta.id).length > 0}
                    onclick={(e) => {
                      e.stopPropagation();
                      if (childrenOf(s.meta.id).length > 0) toggleGroup(s.meta.id);
                    }}
                  >
                    {childrenOf(s.meta.id).length > 0 ? (groupOpen(s.meta.id) ? '▾' : '▸') : ''}
                  </button>
                  {#if renaming === s.meta.id}
                    <input
                      class="t rename"
                      bind:this={renameInput}
                      aria-label="rename session"
                      value={renameText}
                      onblur={() => commitRename(s.meta.id)}
                      onkeydown={(e) => {
                        e.stopPropagation();
                        if (e.key === 'Enter') commitRename(s.meta.id);
                        else if (e.key === 'Escape') renaming = null;
                      }}
                      onmousedown={(e) => e.stopPropagation()}
                      onclick={(e) => e.stopPropagation()}
                    />
                  {:else}
                    <span class="t">{s.meta.title ?? s.meta.id}</span>
                  {/if}
                  {#if sessionRunning(s)}<span class="badge running"><span class="dot"></span>running</span>{/if}
                  <span class="mru">{fmtAgo(s.mru)}</span>
                </div>
                {#if groupOpen(s.meta.id)}
                  {#each childrenOf(s.meta.id) as c (c.meta.id)}
                    <div
                      class="srow child"
                      class:sel={store.current === c.meta.id}
                      role="button"
                      tabindex="0"
                      onclick={() => openSessionById(c.meta.id)}
                      onkeydown={onKey(() => openSessionById(c.meta.id))}
                      ondblclick={() => startRename(c)}
                    >
                      {#if renaming === c.meta.id}
                        <input
                          class="t rename"
                          bind:this={renameInput}
                          aria-label="rename session"
                          value={renameText}
                          onblur={() => commitRename(c.meta.id)}
                          onkeydown={(e) => {
                            e.stopPropagation();
                            if (e.key === 'Enter') commitRename(c.meta.id);
                            else if (e.key === 'Escape') renaming = null;
                          }}
                          onmousedown={(e) => e.stopPropagation()}
                          onclick={(e) => e.stopPropagation()}
                        />
                      {:else}
                        <span class="t">{c.meta.title ?? c.meta.id}</span>
                      {/if}
                      <span class="badge {c.state}"><span class="dot"></span>{c.state}{childInfo(c)?.waiting_on ? ` · ${childInfo(c)?.waiting_on}` : ''}</span>
                      <span class="mru">{fmtAgo(c.mru)}</span>
                    </div>
                    {#each childrenOf(c.meta.id) as gc (gc.meta.id)}
                      <div
                        class="srow nested"
                        class:sel={store.current === gc.meta.id}
                        role="button"
                        tabindex="0"
                        onclick={() => openSessionById(gc.meta.id)}
                        onkeydown={onKey(() => openSessionById(gc.meta.id))}
                        ondblclick={() => startRename(gc)}
                      >
                        {#if renaming === gc.meta.id}
                          <input
                            class="t rename"
                            bind:this={renameInput}
                            aria-label="rename session"
                            value={renameText}
                            onblur={() => commitRename(gc.meta.id)}
                            onkeydown={(e) => {
                              e.stopPropagation();
                              if (e.key === 'Enter') commitRename(gc.meta.id);
                              else if (e.key === 'Escape') renaming = null;
                            }}
                            onmousedown={(e) => e.stopPropagation()}
                            onclick={(e) => e.stopPropagation()}
                          />
                        {:else}
                          <span class="t">{gc.meta.title ?? gc.meta.id}</span>
                        {/if}
                        <span class="badge {gc.state}"><span class="dot"></span>{gc.state}{childInfo(gc)?.waiting_on ? ` · ${childInfo(gc)?.waiting_on}` : ''}</span>
                        <span class="mru">{fmtAgo(gc.mru)}</span>
                      </div>
                    {/each}
                  {/each}
                {/if}
              </div>
            {/each}
          {/if}
        </div>
        <div class="arch">
          <button class="arch-h" type="button" onclick={() => { const q = pane(ws); if (q) q.archOpen = !q.archOpen; }}>
            ⧉ archive {p.archOpen ? '▾' : '▸'}
          </button>
          {#if p.archOpen}
            {#if archived.length === 0}
              <div class="srow"><span class="t none">nothing archived</span></div>
            {:else}
              {#each archived as s (s.meta.id)}
                <div
                  class="srow child"
                  class:sel={store.current === s.meta.id}
                  role="button"
                  tabindex="0"
                  onclick={() => openSessionById(s.meta.id)}
                  onkeydown={onKey(() => openSessionById(s.meta.id))}
                  ondblclick={() => startRename(s)}
                >
                  {#if renaming === s.meta.id}
                    <input
                      class="t rename"
                      bind:this={renameInput}
                      aria-label="rename session"
                      value={renameText}
                      onblur={() => commitRename(s.meta.id)}
                      onkeydown={(e) => {
                        e.stopPropagation();
                        if (e.key === 'Enter') commitRename(s.meta.id);
                        else if (e.key === 'Escape') renaming = null;
                      }}
                      onmousedown={(e) => e.stopPropagation()}
                      onclick={(e) => e.stopPropagation()}
                    />
                  {:else}
                    <span class="t">{s.meta.title ?? s.meta.id}</span>
                  {/if}
                  <span class="mru">{fmtAgo(s.mru)}</span>
                </div>
              {/each}
            {/if}
          {/if}
        </div>
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
    color: #4d5462;
    line-height: 1.6;
  }
  .tree {
    padding-top: 4px;
  }
  .node {
    margin-bottom: 2px;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4.5px 12px;
    cursor: pointer;
    font-size: 12.5px;
  }
  .srow:hover {
    background: var(--panel2);
  }
  .srow.sel {
    background: rgba(76, 194, 255, 0.08);
  }
  .srow.child {
    padding-left: 30px;
  }
  .srow.nested {
    padding-left: 48px;
    font-size: 12px;
  }
  .srow .t {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .srow .t.rename {
    padding: 1px 4px;
    border: 1px solid var(--accent);
    border-radius: 3px;
    background: var(--panel);
    color: var(--fg);
    font: inherit;
  }
  .srow .t.none {
    color: #4d5462;
  }
  .srow .chev {
    font-size: 9px;
    color: var(--dim);
    width: 10px;
    flex: none;
  }
  .srow .chev:not(.has) {
    visibility: hidden;
  }
  .mru {
    font: 9.5px var(--mono);
    color: #4d5462;
    flex: none;
  }
  .badge {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 9.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 7px;
    border: 1px solid;
    border-radius: 9px;
  }
  .badge .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: currentColor;
  }
  .badge.running {
    color: var(--acc);
    border-color: rgba(76, 194, 255, 0.4);
  }
  .badge.running .dot {
    animation: pulse 1.2s ease-in-out infinite;
  }
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
  .arch {
    border-top: 1px solid var(--line);
    margin-top: 8px;
    flex: none;
  }
  .arch-h {
    width: 100%;
    text-align: left;
    padding: 8px 12px;
    font: 10.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--dim);
  }
  .empty {
    padding: 24px;
    text-align: center;
    font: 11px var(--mono);
    color: #4d5462;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
