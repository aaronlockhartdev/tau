<script lang="ts">
  // Right pane (spec §9): tabbed `sub-agents` | `plan`. The sub-agents
  // panel groups rows by lifecycle (live on top, history collapsed behind a
  // count header; the per-row badge already carries the exact state), and
  // each row carries a compact projection of the child's plan — the fleet
  // view (design: N/M + current step + gate verdict). The plan tab renders
  // the session's own parsed plan file (spec §5.3). Last-modified sorted;
  // the panel renders a tree (nesting supported) — double-click opens the
  // sub-agent's session, which is an ordinary session (a tab opens for it).
  import { store, pane, ensurePane, switchSession, currentSession, setFollow, type PaneState } from '../lib/store.svelte';
  import { fmtK } from '../lib/format';
  import { onActivate } from '../lib/ui';
  import { ChevronDown } from '@lucide/svelte';
  import type { SubagentInfo } from '../lib/protocol';
  import { fleetGate, planProgress } from '../lib/plan';
  import PlanView from './PlanView.svelte';

  const cur = $derived(currentSession());
  const ws = $derived(cur?.meta.workspace ?? null);
  const p = $derived<PaneState | null>(pane(ws));
  $effect(() => {
    if (ws) ensurePane(ws);
  });


  const subs = $derived(cur ? cur.subagents : []);
  const isLive = (i: SubagentInfo) => i.state === 'running' || i.state === 'idle';
  // Crew order (P3): running first, then waiting, then terminal — last-modified
  // the ties within a rank.
  const crewRank = (i: SubagentInfo) => (i.state === 'running' ? 0 : i.state === 'idle' ? 1 : 2);
  const byCrew = (a: SubagentInfo, b: SubagentInfo) =>
    crewRank(a) - crewRank(b) || childLastModified(b) - childLastModified(a);
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

  function childLastModified(i: SubagentInfo): number {
    return store.sessions[i.child]?.last_modified ?? 0;
  }
  function titleOf(i: SubagentInfo): string {
    return store.sessions[i.child]?.meta.title ?? i.child;
  }
  function toggleHist(): void {
    const q = pane(ws);
    if (q) q.historyOpen = !q.historyOpen;
  }
  function selectSub(h: string): void {
    const q = pane(ws);
    if (q) q.selSub = h;
  }

  // The session's plan (the plan tab): the store's `plan` field — the
  // snapshot seeds it, plan_changed keeps it live.
  const plan = $derived(cur?.plan ?? { steps: [], criteria: '', notes: '' });
  const gateFailed = $derived(cur?.gate_failed ?? false);

  // The fleet projection (design: a compact child plan in the parent's
  // view): N/M + current step + the gate's verdict from the child's
  // observed lifecycle. The child's full plan lives in the child's tab.
  function planLine(r: SubagentInfo): string | null {
    const child = store.sessions[r.child]?.plan;
    if (!child || child.steps.length === 0) return null;
    const { done, total, current } = planProgress(child);
    const gate = fleetGate(r.state);
    const gateLabel = gate === 'pass' ? 'gate ✓' : gate === 'fail' ? 'gate ✗' : 'gate …';
    return current ? `${done}/${total} · ${current} · ${gateLabel}` : `${done}/${total} · ${gateLabel}`;
  }
</script>

<div class="pane">
  <div class="tabs" role="tablist">
    <div class="seg">
      <button class="tab" class:on={p?.rtab === 'subs'} disabled={!p} onclick={() => { if (p) p.rtab = 'subs'; }}>sub-agents</button>
      <button class="tab" class:on={p?.rtab === 'plan'} disabled={!p} onclick={() => { if (p) p.rtab = 'plan'; }}>plan</button>
    </div>
  </div>
  {#if p}
    {#if p.rtab === 'plan'}
      <PlanView plan={plan} gateFailed={gateFailed} />
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
            {#if planLine(r)}
              <div class="l3">{planLine(r)}</div>
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
    background: var(--hover);
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
  /* The fleet projection: the child's plan, compact. */
  .crewrow .l3 {
    margin: 2px 0 0 19px;
    font: 10px var(--mono);
    color: var(--faint);
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
  .badge.stopped {
    color: var(--dim);
  }
  .badge.stopped .dot {
    background: var(--dim);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
</style>
