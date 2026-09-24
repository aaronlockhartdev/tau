<script lang="ts">
  // Right pane (spec §9, the #11 verdict): tabbed tasks | sub-agents.
  // F3: no filter chips — each tab groups rows by lifecycle (active/live
  // on top, history collapsed behind a count header; the per-row badge
  // already carries the exact state). Last-modified sorted.
  // The tasks panel shows only the currently opened session's tasks
  // (per-session tasks, spec §5.3); a task row expands into labeled
  // detail sections. The sub-agents panel renders a tree (nesting
  // supported) — double-click opens the sub-agent's session, which is an
  // ordinary session (a tab opens for it).
  import { store, pane, ensurePane, openSessionById, type PaneState } from '../lib/store.svelte';
  import type { Task, SubagentInfo } from '../lib/protocol';

  const cur = $derived(store.current ? store.sessions[store.current] : null);
  const ws = $derived(cur?.meta.workspace ?? null);
  const p = $derived<PaneState | null>(pane(ws));
  $effect(() => {
    if (ws) ensurePane(ws);
  });

  const tasks = $derived(cur ? cur.tasks : []);
  // F3 grouping: active = not done; history = done. The history section
  // collapses behind its count header (p.historyOpen).
  const byUpdated = (a: Task, b: Task) => b.updated - a.updated;
  const liveTasks = $derived(tasks.filter((t) => t.status !== 'done').sort(byUpdated));
  const histTasks = $derived(tasks.filter((t) => t.status === 'done').sort(byUpdated));

  const subs = $derived(cur ? cur.subagents : []);
  const isLive = (i: SubagentInfo) => i.state === 'running' || i.state === 'idle';
  const byMru = (a: SubagentInfo, b: SubagentInfo) => childMru(b) - childMru(a);
  const liveSubs = $derived(subs.filter(isLive).sort(byMru));
  const histSubs = $derived(subs.filter((i) => !isLive(i)).sort(byMru));
  const grandOf = (list: SubagentInfo[], liveOnly: boolean) => {
    const map: Record<string, SubagentInfo[]> = {};
    for (const r of list) {
      const child = store.sessions[r.child];
      const kids = child?.subagents ?? [];
      map[r.handle] = (liveOnly ? kids.filter(isLive) : kids.filter((i) => !isLive(i))).sort(byMru);
    }
    return map;
  };
  const liveGrand = $derived.by(() => grandOf(liveSubs, true));
  const histGrand = $derived.by(() => grandOf(histSubs, false));

  function childMru(i: SubagentInfo): number {
    return store.sessions[i.child]?.mru ?? 0;
  }
  function titleOf(i: SubagentInfo): string {
    return store.sessions[i.child]?.meta.title ?? i.child;
  }
  function setRtab(t: PaneState['rtab']): void {
    const q = pane(ws);
    if (q) q.rtab = t;
  }
  function toggleHist(): void {
    const q = pane(ws);
    if (q) q.historyOpen = !q.historyOpen;
  }
  function toggleTask(id: string): void {
    const q = pane(ws);
    if (!q) return;
    q.expandedTasks = q.expandedTasks.includes(id) ? q.expandedTasks.filter((x) => x !== id) : [...q.expandedTasks, id];
  }
  function selectSub(h: string): void {
    const q = pane(ws);
    if (q) q.selSub = h;
  }
  const onKey = (fn: () => void) => (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      fn();
    }
  };
  const fmtAgo = (ms: number) => {
    const m = (Date.now() - ms) / 60000;
    return m < 1 ? 'just now' : m < 60 ? `${Math.round(m)}m` : `${Math.round(m / 60)}h`;
  };
</script>

<div class="pane">
  <div class="tabs" role="tablist">
    <button class="tab" class:on={p?.rtab === 'tasks'} disabled={!p} onclick={() => { if (p) setRtab('tasks'); }}>tasks</button>
    <button class="tab" class:on={p?.rtab === 'subs'} disabled={!p} onclick={() => { if (p) setRtab('subs'); }}>sub-agents</button>
  </div>
  {#if p}
  {#if p.rtab === 'tasks'}
    <div class="list">
      {#if liveTasks.length === 0 && histTasks.length === 0}
        <div class="emptyc">No tasks — ask the agent to make one</div>
      {:else}
        {#snippet taskRow(t: Task)}
          <div
            class="lrow"
            class:open={p?.expandedTasks.includes(t.id)}
            role="button"
            tabindex="0"
            onclick={() => toggleTask(t.id)}
            onkeydown={onKey(() => toggleTask(t.id))}
          >
            <div class="lh">
              <svg class="chev" class:open={p?.expandedTasks.includes(t.id)}><use href="#i-chev"/></svg>
              <span class="badge {t.status === 'in_progress' ? 'in-progress' : t.status}">
                <span class="dot"></span>{t.status}
              </span>
              <span class="ln">{t.title}</span>
              <span class="lm">{t.worker?.session ?? ''}</span>
            </div>
            {#if p?.expandedTasks.includes(t.id)}
              <div class="ld">
                {#if t.steps.length > 0}
                  <div class="dl">steps — {t.steps.filter((s) => s.status === 'done').length}/{t.steps.length} done</div>
                  {#each t.steps as s (s.text)}
                    <div class="step {s.status}">
                      <span class="mk">{s.status === 'done' ? '✓' : s.status === 'active' ? '▸' : '○'}</span>{s.text}
                      {#if s.expected_output}<span class="eo">→ {s.expected_output}</span>{/if}
                    </div>
                  {/each}
                {/if}
                {#if t.criteria.length > 0}
                  <div class="dl">acceptance criteria</div>
                  {#each t.criteria as c (c.text)}
                    {@const sat = c.status === 'satisfied'}
                    <div class="ev"><span class:ok={sat} class:pend={!sat}>{sat ? '✓' : '…'}</span> {c.text}</div>
                  {/each}
                {/if}
                {#if t.evidence.length > 0}
                  <div class="dl">evidence</div>
                  {#each t.evidence as e (e.summary)}
                    <div class="ev"><span class:ok={e.passed} class:pend={!e.passed}>{e.passed ? '✓' : '…'}</span>
                      {e.summary}{e.command ? ` — ${e.command}` : ''}</div>
                  {/each}
                {/if}
                {#if t.blockers.length > 0}
                  <div class="dl">blocker</div>
                  <div class="block">
                    ⛔ {t.blockers.map((b) => `${b.reason}${b.needs ? ` — ${b.needs}` : ''}`).join(' · ')}
                  </div>
                {/if}
                <div class="a">
                  {t.worker ? `assigned: ${t.worker.session} (${t.worker.status})` : 'unassigned'} · modified {fmtAgo(t.updated)}
                </div>
              </div>
            {/if}
          </div>
        {/snippet}
        {#if liveTasks.length > 0}
          <div class="ghead">active · {liveTasks.length}</div>
        {/if}
        {#each liveTasks as t (t.id)}
          {@render taskRow(t)}
        {/each}
        {#if histTasks.length > 0}
          <div class="ghead" class:open={p.historyOpen} role="button" tabindex="0" onclick={toggleHist} onkeydown={onKey(toggleHist)}>
            <svg class="chev"><use href="#i-chev"/></svg>history · {histTasks.length}
          </div>
          {#if p.historyOpen}
            {#each histTasks as t (t.id)}
              {@render taskRow(t)}
            {/each}
          {/if}
        {/if}
      {/if}
    </div>
  {:else}
    <div class="list">
      {#if liveSubs.length === 0 && histSubs.length === 0}
        <div class="emptyc">No sub-agents — ask the agent to spawn one</div>
      {:else}
        {#if liveSubs.length > 0}
          <div class="ghead">live · {liveSubs.length}</div>
        {/if}
        {#each liveSubs as r (r.handle)}
          <div
            class="srow2"
            class:sel={p.selSub === r.handle}
            role="button"
            tabindex="0"
            onclick={() => selectSub(r.handle)}
            onkeydown={onKey(() => selectSub(r.handle))}
            ondblclick={() => openSessionById(r.child)}
          >
            <span class="badge {r.state}"><span class="dot"></span>{r.state}{r.waiting_on ? ` · ${r.waiting_on}` : ''}</span>
            <span class="ln">{titleOf(r)}</span>
            <span class="lm">{r.task?.id ?? ''}</span>
          </div>
          {#each liveGrand[r.handle] ?? [] as g (g.handle)}
            <div
              class="srow2 d2"
              class:sel={p.selSub === g.handle}
              role="button"
              tabindex="0"
              onclick={() => selectSub(g.handle)}
              onkeydown={onKey(() => selectSub(g.handle))}
              ondblclick={() => openSessionById(g.child)}
            >
              <span class="badge {g.state}"><span class="dot"></span>{g.state}{g.waiting_on ? ` · ${g.waiting_on}` : ''}</span>
              <span class="ln">{titleOf(g)}</span>
              <span class="lm">{g.task?.id ?? ''}</span>
            </div>
          {/each}
        {/each}
        {#if histSubs.length > 0}
          <div class="ghead" class:open={p.historyOpen} role="button" tabindex="0" onclick={toggleHist} onkeydown={onKey(toggleHist)}>
            <svg class="chev"><use href="#i-chev"/></svg>history · {histSubs.length}
          </div>
          {#if p.historyOpen}
            {#each histSubs as r (r.handle)}
              <div
                class="srow2"
                class:sel={p.selSub === r.handle}
                role="button"
                tabindex="0"
                onclick={() => selectSub(r.handle)}
                onkeydown={onKey(() => selectSub(r.handle))}
                ondblclick={() => openSessionById(r.child)}
              >
                <span class="badge {r.state}"><span class="dot"></span>{r.state}{r.waiting_on ? ` · ${r.waiting_on}` : ''}</span>
                <span class="ln">{titleOf(r)}</span>
                <span class="lm">{r.task?.id ?? ''}</span>
              </div>
              {#each histGrand[r.handle] ?? [] as g (g.handle)}
                <div
                  class="srow2 d2"
                  class:sel={p.selSub === g.handle}
                  role="button"
                  tabindex="0"
                  onclick={() => selectSub(g.handle)}
                  onkeydown={onKey(() => selectSub(g.handle))}
                  ondblclick={() => openSessionById(g.child)}
                >
                  <span class="badge {g.state}"><span class="dot"></span>{g.state}{g.waiting_on ? ` · ${g.waiting_on}` : ''}</span>
                  <span class="ln">{titleOf(g)}</span>
                  <span class="lm">{g.task?.id ?? ''}</span>
                </div>
              {/each}
            {/each}
          {/if}
        {/if}
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
  .ghead {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px 4px;
    font: 10px var(--mono);
    letter-spacing: 0.08em;
    color: var(--faint);
  }
  .ghead[role='button'] {
    cursor: pointer;
  }
  .ghead .chev {
    width: 10px;
    height: 10px;
    flex: none;
    transition: transform 0.12s;
  }
  .ghead.open .chev {
    transform: rotate(90deg);
  }
  .list {
    display: flex;
    flex-direction: column;
    padding: 4px 0;
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
  .emptyc {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    font: 11px var(--mono);
    color: var(--faint);
    text-align: center;
    padding: 0 16px;
  }
  .lrow {
    padding: 7px 14px;
    cursor: pointer;
    border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent);
  }
  .lrow:hover {
    background: var(--panel2);
  }
  .lrow .lh {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .lrow .ln {
    flex: 1;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lrow .lm {
    font: 9.5px var(--mono);
    color: var(--faint);
  }
  .lrow .chev {
    width: 10px;
    height: 10px;
    flex: none;
    color: var(--dim);
    transition: transform 0.12s;
  }
  .lrow .chev.open {
    transform: rotate(90deg);
  }
  .lrow .ld {
    margin-top: 8px;
  }
  .ld .dl {
    font: 9.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--faint);
    margin: 6px 0 2px;
  }
  .ld .dl:first-child {
    margin-top: 0;
  }
  .ld .step {
    display: flex;
    gap: 8px;
    font-size: 12px;
    padding: 1px 0;
    color: var(--dim);
  }
  .ld .step .mk {
    font-family: var(--mono);
  }
  .ld .step.active {
    color: var(--tx);
  }
  .ld .step.active .mk {
    color: var(--acc);
  }
  .ld .step.done .mk {
    color: var(--green);
  }
  .ld .step .eo {
    color: var(--faint);
  }
  .ld .ev {
    font: 10.5px var(--mono);
    color: var(--dim);
    padding: 1px 0;
  }
  .ld .ev .ok {
    color: var(--green);
  }
  .ld .ev .pend {
    color: var(--amber);
  }
  .ld .block {
    color: var(--red);
    font-size: 12px;
  }
  .ld .a {
    margin-top: 6px;
    font: 10.5px var(--mono);
    color: var(--faint);
  }
  .srow2 {
    padding: 7px 14px;
    cursor: pointer;
    border-bottom: 1px solid color-mix(in srgb, var(--line) 50%, transparent);
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .srow2:hover {
    background: var(--panel2);
  }
  .srow2.sel {
    background: color-mix(in srgb, var(--acc) 12%, transparent);
  }
  .srow2 .ln {
    flex: 1;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .srow2 .lm {
    font: 9.5px var(--mono);
    color: var(--faint);
  }
  .srow2.d2 {
    padding-left: 34px;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font: 10.5px/1.6 var(--mono);
    padding: 1px 7px;
    border-radius: 9px;
    border: 1px solid var(--line);
    color: var(--dim);
    white-space: nowrap;
    flex: none;
  }
  .badge .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--dim);
  }
  .badge.running,
  .badge.in-progress {
    color: var(--acc);
    border-color: color-mix(in srgb, var(--acc) 40%, transparent);
  }
  .badge.running .dot,
  .badge.in-progress .dot {
    background: var(--acc);
    animation: pulse 1.2s infinite;
  }
  .badge.done {
    color: var(--green);
    border-color: color-mix(in srgb, var(--green) 40%, transparent);
  }
  .badge.done .dot {
    background: var(--green);
  }
  .badge.idle {
    color: var(--amber);
    border-color: color-mix(in srgb, var(--amber) 40%, transparent);
  }
  .badge.idle .dot {
    background: var(--amber);
  }
  .badge.failed,
  .badge.blocked {
    color: var(--red);
    border-color: color-mix(in srgb, var(--red) 45%, transparent);
  }
  .badge.failed .dot {
    background: var(--red);
    animation: pulse 0.8s infinite;
  }
  .badge.blocked .dot {
    background: var(--red);
  }
  .badge.stopped,
  .badge.pending {
    color: var(--dim);
  }
  .badge.stopped .dot,
  .badge.pending .dot {
    background: var(--dim);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
