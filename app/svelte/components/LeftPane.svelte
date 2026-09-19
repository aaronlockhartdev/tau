<script lang="ts">
  // Left pane (spec §9, the #11 verdict): tabbed files | sessions. The
  // sessions tab is the MRU-sorted session tree — sub-agent sessions
  // grouped under their parent (collapsible; the active session's group
  // expanded by default), an archive folder at the bottom, and the badge
  // rule: top-level rows are badged `running` only while the model is
  // generating or a sub-agent is running (inactive = untagged); sub-agent
  // rows keep their full lifecycle tags.
  import { store, pane, ensurePane, openSessionById, type PaneState } from '../lib/store.svelte';
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

  // The demo's file tree (the prototype's fixture). The v0 protocol has a
  // file_read command but no directory listing, so the live pane shows a
  // note instead of a fake tree.
  type FileNode = { n?: string; d?: number; ch?: FileNode[]; f?: string };
  const DEMO_TREE: FileNode[] = [
    {
      n: 'src',
      d: 1,
      ch: [
        { n: 'protocol', d: 1, ch: [{ f: 'message.rs' }, { f: 'sse.rs' }, { f: 'envelope.rs' }] },
        { f: 'loop.rs' },
        { f: 'main.rs' }
      ]
    },
    {
      n: 'docs',
      d: 1,
      ch: [
        {
          n: 'adr',
          d: 1,
          ch: [
            { f: '0001-native-subagents-and-task-handoff.md' },
            { f: '0004-om-based-compaction.md' },
            { f: '0006-core-gui-protocol-single-message-crate.md' }
          ]
        },
        { f: 'v0.md' }
      ]
    },
    { f: 'CONTEXT.md' },
    { f: 'AGENTS.md' },
    { f: 'Cargo.toml' }
  ];
  let openDirs = new Set<string>(['src/', 'src/protocol/', 'docs/']);
  function toggleDir(path: string): void {
    openDirs.has(path) ? openDirs.delete(path) : openDirs.add(path);
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
    return q.openGroups.length === 0 && active?.meta.id === sid;
  }
  function toggleGroup(sid: string): void {
    const q = pane(ws);
    if (!q) return;
    if (q.openGroups.length === 0) q.openGroups = active ? [active.meta.id] : [];
    q.openGroups = groupOpen(sid) ? q.openGroups.filter((x) => x !== sid) : [...q.openGroups, sid];
  }
</script>

<div class="pane">
  {#if p}
    <div class="tabs" role="tablist">
      <button class="tab" class:on={p.ltab === 'files'} onclick={() => setLtab('files')}>files</button>
      <button class="tab" class:on={p.ltab === 'sessions'} onclick={() => setLtab('sessions')}>sessions</button>
    </div>

    {#if p.ltab === 'files'}
      <div class="sec">
        {#if store.demo}
          <div class="fbox">
            {#each DEMO_TREE as it, i (i)}
              {#if it.d}
                {@const path = it.n + '/'}
                <div class="frow">
                  <button class="chev" type="button" onclick={() => toggleDir(path)}
                    >{openDirs.has(path) ? '▾' : '▸'}</button
                  ><span class="dir">{it.n}/</span>
                </div>
                {#if openDirs.has(path) && it.ch}
                  <div class="fbox">
                    {#each it.ch as sub, j (j)}
                      {#if sub.d}
                        {@const spath = path + sub.n + '/'}
                        <div class="frow d1">
                          <button class="chev" type="button" onclick={() => toggleDir(spath)}
                            >{openDirs.has(spath) ? '▾' : '▸'}</button
                          ><span class="dir">{sub.n}/</span>
                        </div>
                        {#if openDirs.has(spath) && sub.ch}
                          <div class="fbox d2">
                            {#each sub.ch as leaf, k (k)}
                              <div class="frow d2"><span class="chev hide"></span><span class="fn">{leaf.f}</span></div>
                            {/each}
                          </div>
                        {/if}
                      {:else}
                        <div class="frow d1"><span class="chev hide"></span><span class="fn">{sub.f}</span></div>
                      {/if}
                    {/each}
                  </div>
                {/if}
              {:else}
                <div class="frow"><span class="chev hide"></span><span class="fn">{it.f}</span></div>
              {/if}
            {/each}
          </div>
        {:else}
          <div class="note">v0 has no directory-listing command — the file tree shows in the demo only.</div>
        {/if}
      </div>
    {:else}
      <div class="sec">
        <div class="tree">
          {#if top.length === 0}
            <div class="empty"><span class="big">no sessions yet</span></div>
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
                  <span class="t">{s.meta.title ?? s.meta.id}</span>
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
                    >
                      <span class="t">{c.meta.title ?? c.meta.id}</span>
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
                      >
                        <span class="t">{gc.meta.title ?? gc.meta.id}</span>
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
                >
                  <span class="t">{s.meta.title ?? s.meta.id}</span>
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
  .fbox {
    padding-left: 12px;
  }
  .frow {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2.5px 12px;
    font-size: 12px;
    color: var(--dim);
  }
  .frow.d1 {
    padding-left: 26px;
  }
  .frow.d2 {
    padding-left: 40px;
  }
  .frow .dir {
    color: var(--dim);
  }
  .frow .fn {
    color: var(--tx);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .frow .chev {
    font-size: 9px;
    color: var(--dim);
    width: 10px;
    flex: none;
  }
  .frow .chev.hide {
    visibility: hidden;
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
    color: var(--err);
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
