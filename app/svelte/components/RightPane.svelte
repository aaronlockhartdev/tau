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
  import { store, pane, ensurePane, switchSession, currentSession, setFollow, type PaneState } from '../lib/store.svelte';
  import { fmtAgo, fmtK } from '../lib/format';
  import { onActivate } from '../lib/ui';
  import { ChevronDown, CircleSlash } from '@lucide/svelte';
  import type { Task, SubagentInfo } from '../lib/protocol';

  const cur = $derived(currentSession());
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
  // Crew order (P3): running first, then waiting, then terminal — MRU breaks
  // the ties within a rank.
  const crewRank = (i: SubagentInfo) => (i.state === 'running' ? 0 : i.state === 'idle' ? 1 : 2);
  const byCrew = (a: SubagentInfo, b: SubagentInfo) =>
    crewRank(a) - crewRank(b) || childMru(b) - childMru(a);
  const liveSubs = $derived(subs.filter(isLive).sort(byCrew));
  const histSubs = $derived(subs.filter((i) => !isLive(i)).sort(byCrew));
  const grandOf = (list: SubagentInfo[], liveOnly: boolean) => {
    const map: Record<string, SubagentInfo[]> = {};
    for (const r of list) {
      const child = store.sessions[r.child];
      const kids = child?.subagents ?? [];
      map[r.handle] = (liveOnly ? kids.filter(isLive) : kids.filter((i) => !isLive(i))).sort(byCrew);
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
</script>

<div class="pane">
  <div class="tabs" role="tablist">
    <div class="seg">
      <button class="tab" class:on={p?.rtab === 'tasks'} disabled={!p} onclick={() => { if (p) setRtab('tasks'); }}>tasks</button>
      <button class="tab" class:on={p?.rtab === 'subs'} disabled={!p} onclick={() => { if (p) setRtab('subs'); }}>sub-agents</button>
    </div>
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
            onkeydown={onActivate(() => toggleTask(t.id))}
          >
            <div class="lh">
              <ChevronDown class="chev {p?.expandedTasks.includes(t.id) ? 'open' : ''}" size={10} />
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
                    <span class="bmark"><CircleSlash class="bi" size={12} aria-hidden="true" /></span>{t.blockers.map((b) => `${b.reason}${b.needs ? ` — ${b.needs}` : ''}`).join(' · ')}
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
          <div class="ghead" class:open={p.historyOpen} role="button" tabindex="0" onclick={toggleHist} onkeydown={onActivate(toggleHist)}>
            <ChevronDown class="chev" size={10} />history · {histTasks.length}
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
        {#snippet subRow(r: SubagentInfo, deep: boolean)}
          <div
            class="srow2 crewrow"
            class:d2={deep}
            class:sel={p.selSub === r.handle}
            role="button"
            tabindex="0"
            onclick={() => selectSub(r.handle)}
            onkeydown={onActivate(() => selectSub(r.handle))}
            ondblclick={() => void switchSession(r.child)}
          >
            <div class="l1">
              <span class="badge {r.state}"><span class="dot"></span>{r.state}{r.waiting_on ? ` · ${r.waiting_on}` : ''}</span>
              <span class="ln">{titleOf(r)}</span>
              <span class="chip">{r.agent_type}</span>
              <span class="chip dim">{r.context_mode}</span>
              {#if r.usage}<span class="lm">{fmtK(r.usage.input_tokens)} in · {fmtK(r.usage.output_tokens)} out</span>{/if}
              <span class="acts">
                <button
                  type="button"
                  class="act"
                  class:on={store.follow === r.handle}
                  title="follow this worker in the transcript"
                  onclick={(e) => {
                    e.stopPropagation();
                    setFollow(store.follow === r.handle ? null : r.handle);
                  }}
                >{store.follow === r.handle ? 'following' : 'follow'}</button>
                <button
                  type="button"
                  class="act"
                  title="open the child session"
                  onclick={(e) => {
                    e.stopPropagation();
                    void switchSession(r.child);
                  }}
                >open</button>
              </span>
            </div>
            {#if r.last_message}
              <div class="l2">{r.last_message}</div>
            {/if}
          </div>
        {/snippet}
        {#if liveSubs.length > 0}
          <div class="ghead">live · {liveSubs.length}</div>
        {/if}
        {#each liveSubs as r (r.handle)}
          {@render subRow(r, false)}
          {#each liveGrand[r.handle] ?? [] as g (g.handle)}
            {@render subRow(g, true)}
          {/each}
        {/each}
        {#if histSubs.length > 0}
          <div class="ghead" class:open={p.historyOpen} role="button" tabindex="0" onclick={toggleHist} onkeydown={onActivate(toggleHist)}>
            <ChevronDown class="chev" size={10} />history · {histSubs.length}
          </div>
          {#if p.historyOpen}
            {#each histSubs as r (r.handle)}
              {@render subRow(r, false)}
              {#each histGrand[r.handle] ?? [] as g (g.handle)}
                {@render subRow(g, true)}
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
    align-items: center;
    gap: 8px;
    padding: 10px 12px 8px;
    flex: none;
  }
  .seg {
    display: flex;
    gap: 2px;
    padding: 2px;
    background: var(--panel2);
    border: 1px solid var(--line);
    border-radius: 8px;
  }
  .tab {
    padding: 4px 10px;
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
  .ghead {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 8px 5px;
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--faint);
  }
  .ghead[role='button'] {
    cursor: pointer;
  }
  .ghead :global(.chev) {
    flex: none;
    transition: transform 0.12s;
  }
  .ghead.open :global(.chev) {
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
    padding: 6px 8px;
    cursor: pointer;
  }
  .lrow:hover {
    background: rgba(255, 255, 255, 0.04);
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
  .lrow :global(.chev) {
    flex: none;
    color: var(--dim);
    transition: transform 0.12s;
  }
  .lrow :global(.chev.open) {
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
  .ld .block .bmark {
    display: inline-flex;
    margin-right: 4px;
  }
  .ld .block :global(.bi) {
    flex: none;
  }
  .ld .a {
    margin-top: 6px;
    font: 10.5px var(--mono);
    color: var(--faint);
  }
  .srow2 {
    padding: 6px 8px;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 8px;
    border-radius: 8px;
    border: 1px solid transparent;
  }
  .srow2:hover {
    background: rgba(255, 255, 255, 0.04);
  }
  .srow2.sel {
    background: var(--raised);
    border-color: var(--line2);
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
  /* The live crew row (P3): two lines — identity + live line. */
  .crewrow {
    align-items: flex-start;
    position: relative;
  }
  .crewrow .l1 {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    min-width: 0;
  }
  .crewrow .chip {
    flex: none;
    font: 9.5px var(--mono);
    color: var(--dim);
    background: var(--panel3);
    border-radius: 4px;
    padding: 0 5px;
  }
  .crewrow .chip.dim {
    color: var(--faint);
  }
  .crewrow .lm {
    flex: none;
  }
  .crewrow .l2 {
    margin: 2px 0 0 19px;
    font-size: 11px;
    color: var(--dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .crewrow .acts {
    flex: none;
    display: flex;
    gap: 4px;
    margin-left: auto;
    opacity: 0;
  }
  .crewrow:hover .acts {
    opacity: 1;
  }
  .crewrow .act {
    background: none;
    border: 1px solid var(--line2);
    border-radius: 5px;
    padding: 1px 6px;
    font: 10px var(--mono);
    color: var(--dim);
    cursor: pointer;
  }
  .crewrow .act.on {
    color: var(--acc);
    border-color: var(--acc);
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
