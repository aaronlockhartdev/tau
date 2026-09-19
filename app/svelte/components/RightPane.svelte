<script lang="ts">
  // Right pane (spec §9, the #11 verdict): tabbed tasks | sub-agents. Both
  // panels are status-filtered (default: not-done) and last-modified
  // sorted. The tasks panel shows only the currently opened session's
  // tasks (per-session tasks, spec §5.3); a task row expands into labeled
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
  const tFilter = $derived(p?.tFilter ?? 'open');
  const filteredTasks = $derived(
    tasks
      .filter((t) => tFilter === 'all' || (tFilter === 'open' ? t.status !== 'done' : t.status === tFilter))
      .sort((a, b) => b.updated - a.updated)
  );

  const subs = $derived(cur ? cur.subagents : []);
  const sFilter = $derived(p?.sFilter ?? 'open');
  const pass = (i: SubagentInfo) =>
    sFilter === 'all' || (sFilter === 'open' ? i.state !== 'done' : i.state === sFilter);
  const roots = $derived(subs.filter(pass).sort((a, b) => childMru(b) - childMru(a)));
  const grandchildren = $derived.by(() => {
    const map: Record<string, SubagentInfo[]> = {};
    for (const r of roots) {
      const child = store.sessions[r.child];
      map[r.handle] = (child?.subagents ?? []).filter(pass);
    }
    return map;
  });

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
  function setTFilter(f: PaneState['tFilter']): void {
    const q = pane(ws);
    if (q) q.tFilter = f;
  }
  function setSFilter(f: PaneState['sFilter']): void {
    const q = pane(ws);
    if (q) q.sFilter = f;
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
  const T_CHIPS: Array<[PaneState['tFilter'], string]> = [
    ['open', 'open'],
    ['all', 'all'],
    ['in-progress', 'in-progress'],
    ['blocked', 'blocked'],
    ['pending', 'pending'],
    ['done', 'done']
  ];
  const S_CHIPS: Array<[PaneState['sFilter'], string]> = [
    ['open', 'open'],
    ['all', 'all'],
    ['running', 'running'],
    ['idle', 'idle'],
    ['failed', 'failed'],
    ['stopped', 'stopped'],
    ['done', 'done']
  ];
  function contractText(t: Task): string {
    const rc = t.resume_contract;
    if (!rc) return '';
    const lines: string[] = [];
    if (rc.current_step) lines.push(`step: ${rc.current_step.text}\nexpected: ${rc.current_step.expected_output}`);
    if (rc.gaps.length > 0) lines.push(`gaps: ${rc.gaps.join('; ')}`);
    if (rc.blockers.length > 0) lines.push(`blockers: ${rc.blockers.map((b) => b.reason).join('; ')}`);
    lines.push(`next: ${rc.next_action}`);
    return lines.join('\n');
  }
</script>

<div class="pane">
  {#if p}
  <div class="tabs" role="tablist">
    <button class="tab" class:on={p.rtab === 'tasks'} onclick={() => setRtab('tasks')}>tasks</button>
    <button class="tab" class:on={p.rtab === 'subs'} onclick={() => setRtab('subs')}>sub-agents</button>
  </div>

  {#if p.rtab === 'tasks'}
    <div class="filters">
      {#each T_CHIPS as [key, label] (key)}
        <button class="fchip" class:on={tFilter === key} onclick={() => setTFilter(key)}>{label}</button>
      {/each}
    </div>
    <div class="list">
      {#if filteredTasks.length === 0}
        <div class="lrow"><span class="lm">none</span></div>
      {:else}
        {#each filteredTasks as t (t.id)}
          <div
            class="lrow"
            class:open={p.expandedTasks.includes(t.id)}
            role="button"
            tabindex="0"
            onclick={() => toggleTask(t.id)}
            onkeydown={onKey(() => toggleTask(t.id))}
          >
            <div class="lh">
              <span class="chev">{p.expandedTasks.includes(t.id) ? '▾' : '▸'}</span>
              <span class="badge {t.status === 'in_progress' ? 'in-progress' : t.status}">
                <span class="dot"></span>{t.status}
              </span>
              <span class="ln">{t.title}</span>
              <span class="lm">{t.worker?.session ?? ''}</span>
            </div>
            {#if p.expandedTasks.includes(t.id)}
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
                {#if t.resume_contract}
                  <div class="dl">resume contract</div>
                  <pre class="rc">{contractText(t)}</pre>
                {/if}
                <div class="a">
                  {t.worker ? `assigned: ${t.worker.session} (${t.worker.status})` : 'unassigned'} · modified {fmtAgo(t.updated)}
                </div>
              </div>
            {/if}
          </div>
        {/each}
      {/if}
    </div>
  {:else}
    <div class="filters">
      {#each S_CHIPS as [key, label] (key)}
        <button class="fchip" class:on={sFilter === key} onclick={() => setSFilter(key)}>{label}</button>
      {/each}
    </div>
    <div class="list">
      {#if roots.length === 0}
        <div class="lrow"><span class="lm">none</span></div>
      {:else}
        {#each roots as r (r.handle)}
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
          {#each grandchildren[r.handle] ?? [] as g (g.handle)}
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
  .filters {
    display: flex;
    gap: 4px;
    padding: 8px 12px 4px;
    flex-wrap: wrap;
    flex: none;
  }
  .fchip {
    font: 10.5px var(--mono);
    border: 1px solid var(--line);
    border-radius: 11px;
    padding: 2px 9px;
    color: var(--dim);
  }
  .fchip.on {
    color: var(--acc);
    border-color: rgba(76, 194, 255, 0.5);
    background: rgba(76, 194, 255, 0.08);
  }
  .list {
    padding: 4px 0;
    overflow-y: auto;
    min-height: 0;
    flex: 1;
  }
  .lrow {
    padding: 7px 14px;
    cursor: pointer;
    border-bottom: 1px solid rgba(38, 42, 51, 0.5);
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
    color: #4d5462;
  }
  .lrow .chev {
    font-size: 9px;
    color: var(--dim);
    width: 10px;
  }
  .lrow .ld {
    margin-top: 8px;
  }
  .ld .dl {
    font: 9.5px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #4d5462;
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
    color: #4d5462;
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
  .ld .rc {
    background: #0c0d10;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 8px 10px;
    font: 10.5px/1.55 var(--mono);
    color: #9aa3b2;
    white-space: pre-wrap;
    margin: 0;
  }
  .ld .a {
    margin-top: 6px;
    font: 10.5px var(--mono);
    color: #4d5462;
  }
  .srow2 {
    padding: 7px 14px;
    cursor: pointer;
    border-bottom: 1px solid rgba(38, 42, 51, 0.5);
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .srow2:hover {
    background: var(--panel2);
  }
  .srow2.sel {
    background: rgba(76, 194, 255, 0.12);
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
    color: #4d5462;
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
    border-color: rgba(76, 194, 255, 0.4);
  }
  .badge.running .dot,
  .badge.in-progress .dot {
    background: var(--acc);
    animation: pulse 1.2s infinite;
  }
  .badge.done {
    color: var(--green);
    border-color: rgba(87, 217, 122, 0.4);
  }
  .badge.done .dot {
    background: var(--green);
  }
  .badge.idle {
    color: var(--amber);
    border-color: rgba(232, 182, 76, 0.4);
  }
  .badge.idle .dot {
    background: var(--amber);
  }
  .badge.failed,
  .badge.blocked {
    color: var(--red);
    border-color: rgba(239, 106, 106, 0.45);
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
