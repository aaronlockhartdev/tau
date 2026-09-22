<script lang="ts">
  // One node of the session tree (the left pane's sessions tab): the row's
  // content (title, badge, mru) plus its children rendered recursively. The
  // row shell is the shared TreeNode. Collapse state is the pane's
  // openGroups — null is the default view (the group containing the active
  // session expanded), an array is the explicit set the user has toggled.
  import { store, pane, openSessionById, renameSession, type SessionState } from '../lib/store.svelte';
  import { tick } from 'svelte';
  import SessionNode from './SessionNode.svelte';
  import TreeNode from './TreeNode.svelte';

  let {
    session,
    depth = 0,
    ws,
    sessions
  }: { session: SessionState; depth?: number; ws: string; sessions: SessionState[] } = $props();

  const active = $derived(store.current ? store.sessions[store.current] : null);
  const kids = $derived(
    sessions.filter((s) => s.parent === session.meta.id).sort((a, b) => b.mru - a.mru)
  );

  function groupOpen(sid: string): boolean {
    const q = pane(ws);
    if (!q) return false;
    if (q.openGroups !== null) return q.openGroups.includes(sid);
    // Default view: the chain containing the active session — the group is
    // the session itself or an ancestor of it.
    let a: SessionState | null = active;
    while (a) {
      if (a.meta.id === sid) return true;
      const pid = a.parent;
      a = pid ? (sessions.find((s) => s.meta.id === pid) ?? null) : null;
    }
    return false;
  }

  function toggleGroup(sid: string): void {
    const q = pane(ws);
    if (!q) return;
    if (q.openGroups === null) {
      // First explicit toggle: materialize the default view, then apply it.
      const open = sessions.filter((s) => groupOpen(s.meta.id)).map((s) => s.meta.id);
      q.openGroups = groupOpen(sid) ? open.filter((x) => x !== sid) : [...open, sid];
    } else {
      q.openGroups = groupOpen(sid)
        ? q.openGroups.filter((x) => x !== sid)
        : [...q.openGroups, sid];
    }
  }

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
  {#if depth === 0}
    {#if sessionRunning(session)}<span class="badge running"><span class="dot"></span>running</span>{/if}
  {:else}
    <span class="badge {session.state}"><span class="dot"></span>{session.state}{childInfo(session)?.waiting_on ? ` · ${childInfo(session)?.waiting_on}` : ''}</span>
  {/if}
  <span class="mru">{fmtAgo(session.mru)}</span>
{/snippet}
<TreeNode
  {depth}
  expanded={kids.length > 0 ? groupOpen(session.meta.id) : null}
  selected={store.current === session.meta.id}
  onRow={() => openSessionById(session.meta.id)}
  onRowDbl={() => {
    const q = pane(ws);
    if (q) q.renamingId = session.meta.id;
  }}
  onToggle={kids.length > 0 ? () => toggleGroup(session.meta.id) : undefined}
  label={rowLabel}
/>
{#if kids.length > 0 && groupOpen(session.meta.id)}
  <div class="kids">
    {#each kids as c (c.meta.id)}
      <SessionNode session={c} depth={depth + 1} ws={ws} sessions={sessions} />
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
    padding: 1px 4px;
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
