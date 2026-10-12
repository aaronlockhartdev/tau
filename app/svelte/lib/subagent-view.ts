// The parent's view of its crew: the transcript anchor and the live state.
//
// A sub-agent's own entries (spawn/state/notify) are recorded in the CHILD's
// file (ADR-0001 durable provenance) — the parent's transcript carries only
// the `subagent_spawn` tool call. So a crew row in the parent view is a tool
// entry (the spawn point) correlated to the live SubagentInfo by
// (agent_type, context_mode), spawn order breaking multiplicity ties.

import type { SubagentInfo } from './protocol';

// One spawn point in the parent transcript (a subagent_spawn tool entry).
export interface SpawnRef {
  entryId: string;
  agentType: string;
  contextMode: string;
  brief: string | null;
}

type ToolLike = {
  id: string;
  kind: string;
  name?: string;
  args?: Record<string, unknown>;
  text?: string;
};

// The spawn points, in transcript order (the creation order of `entries`).
export function spawnRefsOf(entries: readonly ToolLike[]): SpawnRef[] {
  const refs: SpawnRef[] = [];
  for (const e of entries) {
    if (e.kind !== 'tool' || e.name !== 'subagent_spawn' || !e.args) continue;
    const a = e.args;
    const type = typeof a.type === 'string' ? a.type : '';
    const mode = typeof a.context_mode === 'string' ? a.context_mode : 'fresh';
    const brief =
      typeof a.task === 'string' && a.task
        ? a.task
        : typeof a.brief === 'string'
          ? a.brief
          : null;
    refs.push({ entryId: e.id, agentType: type, contextMode: mode, brief });
  }
  return refs;
}

// Claim each spawn point to a live crew member: first-come, first-served on
// (agent_type, context_mode), spawn order first. A spawn with no live
// match (the session is cold) claims null — the block renders its static
// spawn marker.
export function claimCrew(
  refs: readonly SpawnRef[],
  infos: readonly SubagentInfo[]
): Map<string, SubagentInfo | null> {
  const free = infos.map((i) => ({ i, taken: false }));
  const out = new Map<string, SubagentInfo | null>();
  for (const r of refs) {
    const hit = free.find(
      (f) =>
        !f.taken && f.i.agent_type === r.agentType && f.i.context_mode === r.contextMode
    );
    if (hit) {
      hit.taken = true;
      out.set(r.entryId, hit.i);
    } else {
      out.set(r.entryId, null);
    }
  }
  return out;
}

// The block's display phase. `waiting-user` is the red "needs you" case —
// the single most important state in the app.
export type CrewPhase =
  | 'spawn'
  | 'running'
  | 'waiting-user'
  | 'waiting-parent'
  | 'done'
  | 'failed'
  | 'stopped';

export interface CrewView {
  phase: CrewPhase;
  agentType: string;
  contextMode: string;
  brief: string | null;
  lastMessage: string | null;
  inputTokens: number | null;
  outputTokens: number | null;
  waitingOn: string | null;
  handle: string | null;
  child: string | null;
}

// The block content for one spawn point: the live info when claimed, the
// static spawn marker otherwise.
export function crewView(ref: SpawnRef, info: SubagentInfo | null): CrewView {
  if (!info) {
    return {
      phase: 'spawn',
      agentType: ref.agentType,
      contextMode: ref.contextMode,
      brief: ref.brief,
      lastMessage: null,
      inputTokens: null,
      outputTokens: null,
      waitingOn: null,
      handle: null,
      child: null
    };
  }
  let phase: CrewPhase;
  if (info.state === 'idle') {
    phase = info.waiting_on === 'user' ? 'waiting-user' : 'waiting-parent';
  } else {
    phase = info.state;
  }
  return {
    phase,
    agentType: ref.agentType,
    contextMode: ref.contextMode,
    brief: ref.brief,
    lastMessage: info.last_message,
    inputTokens: info.usage?.input_tokens ?? null,
    outputTokens: info.usage?.output_tokens ?? null,
    waitingOn: info.waiting_on,
    handle: info.handle,
    child: info.child
  };
}
