<script lang="ts">
  // One node of the session tree (the left pane's sessions tab): the row's
  // content (title, badge, mru) plus its children rendered recursively. The
  // row shell is the shared TreeNode. Collapse state is the pane's
  // openGroups — null is the default view (the group containing the active
  // session expanded), an array is the explicit set the user has toggled.
  import {
    store,
    pane,
    openSessionById,
    renameSession,
    archiveSession,
    restoreSession,
    type SessionState
  } from '../lib/store.svelte';
  import { groupIsOpen } from '../lib/sessions';
  import { tick } from 'svelte';
  import SessionNode from './SessionNode.svelte';
  import TreeNode from './TreeNode.svelte';

  let {
    session,
    depth = 0,
    ws,
    sessions,
    rangeBetween
  }: {
    session: SessionState;
    depth?: number;
    ws: string;
    sessions: SessionState[];
    // The visible-order range helper (the left pane owns the flat order
    // a shift range spans).
    rangeBetween: (a: string, b: string) => string[];
  } = $props();

  const active = $derived(store.current ? store.sessions[store.current] : null);
  const kids = $derived(
    sessions.filter((s) => s.parent === session.meta.id).sort((a, b) => b.mru - a.mru)
  );

  function groupOpen(sid: string, s: SessionState = session): boolean {
    const q = pane(ws);
    if (!q) return false;
    return groupIsOpen(q, s, active, sessions);
  }

  function toggleGroup(sid: string): void {
    const q = pane(ws);
    if (!q) return;
    if (q.openGroups === null) {
      // First explicit toggle: materialize the default view, then apply it.
      const open = sessions.filter((s) => groupOpen(s.meta.id, s)).map((s) => s.meta.id);
      q.openGroups = groupOpen(sid) ? open.filter((x) => x !== sid) : [...open, sid];
    } else {
      q.openGroups = groupOpen(sid)
        ? q.openGroups.filter((x) => x !== sid)
        : [...q.openGroups, sid];
    }
  }
  // Row click: plain opens (and clears the selection), cmd/ctrl
  // toggles this row in the selection (mac Cmd / linux Ctrl), shift
  // spans from the anchor. An archived row never opens — it only ever
  // joins the selection.
  function onRowClick(e: MouseEvent): void {
    const q = pane(ws);
    if (!q) return;
    const id = session.meta.id;
    if (e.shiftKey) {
      const anchor = q.selAnchor ?? q.selected[0] ?? id;
      q.selected = rangeBetween(anchor, id);
      return;
    }
    if (e.metaKey || e.ctrlKey) {
      q.selected = q.selected.includes(id)
        ? q.selected.filter((x) => x !== id)
        : [...q.selected, id];
      q.selAnchor = id;
      return;
    }
    q.selected = [];
    q.selAnchor = id;
    if (session.archived) return;
    void openSessionById(id);
  }

  const isSelected = $derived((pane(ws)?.selected ?? []).includes(session.meta.id));
  // Inline rename: a double-clicked title becomes an input (Enter/blur
  // commits, Esc cancels). One rename at a time, owned by the pane.
  let renameText = $state('');
  let renameInput: HTMLInputElement | undefined = $state(undefined);
  $effect(() => {
    const q = pane(ws);
    if (q?.renamingId === session.meta.id) {
      renameText = session.meta.title ?? '';
      void tick().then(() => {
        renameInput?.focus();
        renameInput?.select();
      });
    }
  });
  function commitRename(): void {
    const q = pane(ws);
    if (!q || q.renamingId !== session.meta.id) return;
    q.renamingId = null;
    void renameSession(session.meta.id, renameText);
  }

  // The right-click menu acts on the selection: right-clicking a
  // selected row offers archive/restore for the whole selection (a
  // single row reads as its own one-element selection); the targets
  // collapse to each row's root, since a child archives/restores with
  // its parent (ADR-0005).
  function rootOf(s: SessionState): string {
    let r = s;
    while (r.parent) r = store.sessions[r.parent] ?? r;
    return r.meta.id;
  }
  function menuItems(): { label: string; archive: string[]; restore: string[] }[] {
    const q = pane(ws);
    if (!q) return [];
    const ids = q.selected.includes(session.meta.id) ? q.selected : [session.meta.id];
    const sel = ids.map((id) => store.sessions[id]).filter((s): s is SessionState => s !== undefined);
    const archive = new Set<string>();
    const restore = new Set<string>();
    for (const s of sel) {
      if (s.archived) restore.add(rootOf(s));
      else archive.add(rootOf(s));
    }
    const out: { label: string; archive: string[]; restore: string[] }[] = [];
    if (archive.size)
      out.push({
        label: archive.size > 1 ? `archive (${archive.size})` : 'archive',
        archive: [...archive],
        restore: []
      });
    if (restore.size)
      out.push({
        label: restore.size > 1 ? `restore (${restore.size})` : 'restore',
        archive: [],
        restore: [...restore]
      });
    return out;
  }
  function applyMenuAction(item: { archive: string[]; restore: string[] }): void {
    ctx = null;
    const q = pane(ws);
    if (q) q.selected = [];
    void Promise.all([
      ...item.archive.map((id) => archiveSession(id)),
      ...item.restore.map((id) => restoreSession(ws, id))
    ]);
  }
  let ctx = $state<{ x: number; y: number } | null>(null);
  // The rendered position: the raw click point, clamped inside the
  // viewport after the first render.
  let ctxPos = $state({ x: 0, y: 0 });
  let ctxMenu: HTMLDivElement | undefined = $state(undefined);
  function onContext(e: MouseEvent): void {
    e.preventDefault();
    ctx = { x: e.clientX, y: e.clientY };
    ctxPos = { x: e.clientX, y: e.clientY };
  }
  $effect(() => {
    if (!ctx) return;
    const close = () => (ctx = null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
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
</script>

{#snippet rowLabel()}
  {#if pane(ws)?.renamingId === session.meta.id}
    <input
      class="name rename"
      maxlength={200}
      bind:this={renameInput}
      bind:value={renameText}
      aria-label="rename session"
      onblur={() => commitRename()}
      onkeydown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') commitRename();
        else if (e.key === 'Escape') {
          const q = pane(ws);
          if (q) q.renamingId = null;
        }
      }}
      onmousedown={(e) => e.stopPropagation()}
      onclick={(e) => e.stopPropagation()}
    />
  {:else}
    <span class="name">{session.meta.title ?? session.meta.id}</span>
  {/if}
  {#if !session.archived && depth === 0}
    {#if sessionRunning(session)}<span class="badge running"><span class="dot"></span>running</span>{/if}
  {:else if !session.archived}
    <span class="badge {session.state}"><span class="dot"></span>{session.state}{childInfo(session)?.waiting_on ? ` · ${childInfo(session)?.waiting_on}` : ''}</span>
  {/if}
  <span class="mru">{fmtAgo(session.mru)}</span>
  {#if !session.archived && !session.parent}
    <button
      class="arch-b"
      type="button"
      title="archive"
      aria-label="archive session"
      onmousedown={(e) => e.stopPropagation()}
      onclick={(e) => {
        e.stopPropagation();
        void archiveSession(session.meta.id);
      }}
    >
      <svg class="ci" width="11" height="11"><use href="#i-archive" /></svg>
    </button>
  {/if}
{/snippet}
  <TreeNode
  {depth}
  expanded={kids.length > 0 ? groupOpen(session.meta.id) : null}
  selected={store.current === session.meta.id}
  multi={isSelected}
  dimmed={session.archived}
  onRow={(e) => onRowClick(e)}
  onRowDbl={
    session.archived
      ? undefined
      : () => {
          const q = pane(ws);
          if (q) q.renamingId = session.meta.id;
        }
  }
  onToggle={kids.length > 0 ? () => toggleGroup(session.meta.id) : undefined}
  onContext={onContext}
  label={rowLabel}
/>
{#if ctx}
  <div class="ctxmenu" role="menu" bind:this={ctxMenu} style:left="{ctxPos.x}px" style:top="{ctxPos.y}px">
    {#each menuItems() as item (item.label)}
      <button type="button" role="menuitem" onclick={() => void applyMenuAction(item)}>
        {item.label}
      </button>
    {/each}
  </div>
{/if}
{#if kids.length > 0 && groupOpen(session.meta.id)}
  <div class="kids">
    {#each kids as c (c.meta.id)}
      <SessionNode session={c} depth={depth + 1} ws={ws} sessions={sessions} rangeBetween={rangeBetween} />
    {/each}
  </div>
{/if}

<style>
  .kids {
    display: flex;
    flex-direction: column;
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name.rename {
    box-sizing: border-box;
    height: 1em;
    padding: 0 4px;
    border: 1px solid var(--acc);
    border-radius: 3px;
    background: var(--panel);
    color: var(--tx);
    font: inherit;
  }
  .mru {
    font: 9.5px var(--mono);
    color: var(--faint);
    flex: none;
  }
  .arch-b {
    flex: none;
    display: inline-flex;
    align-items: center;
    padding: 2px;
    border: none;
    border-radius: 3px;
    background: none;
    color: var(--faint);
    opacity: 0.45;
    cursor: pointer;
  }
  .arch-b:hover,
  .arch-b:focus-visible {
    opacity: 1;
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
  .badge {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: 9.5px var(--mono);
    letter-spacing: 0;
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
    border-color: color-mix(in srgb, var(--acc) 40%, transparent);
  }
  .badge.running .dot {
    animation: pulse 1.2s ease-in-out infinite;
  }
  .badge.idle {
    color: var(--amber);
    border-color: color-mix(in srgb, var(--amber) 40%, transparent);
  }
  .badge.done {
    color: var(--green);
    border-color: color-mix(in srgb, var(--green) 40%, transparent);
  }
  .badge.failed {
    color: var(--red);
    border-color: color-mix(in srgb, var(--red) 40%, transparent);
  }
  .badge.stopped {
    color: var(--dim);
    border-color: color-mix(in srgb, var(--dim) 40%, transparent);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
