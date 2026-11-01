// The parent's view of its crew: the transcript anchor and the live state.
//
// A sub-agent's own entries (spawn/state/notify) are recorded in the CHILD's
// file (ADR-0001 durable provenance) — the parent's transcript carries only
// the `subagent_spawn` tool call. So a crew row in the parent view is a tool
// entry (the spawn point) correlated to the live SubagentInfo by
// (agent_type, context_mode), spawn order breaking multiplicity ties.

import type { SubagentInfo, WaitingOn } from '../wire/protocol';
import { badgeLabel, waitingOnOf } from './subagent-state';
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
    const brief = (typeof a.brief === 'string' ? a.brief : null);
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
// the single most important state in the app, and both of its sources read
// from the typed state: an `idle` child whose declared target is the user,
// and an `awaiting` child (the `QUESTION:` marker, spec §5.2). A state this
// build does not know (the `other` arm) keeps its own spelling, so a state a
// newer core adds still renders — the open-union escape `EntryKind` uses.
export type CrewPhase =
  | 'spawn'
  | 'running'
  | 'waiting-user'
  | 'waiting-parent'
  | 'done'
  | 'failed'
  | 'stopped'
  | (string & {});

export interface CrewView {
  phase: CrewPhase;
  // The badge's text: the phase's spelling, `waiting on you` for the red
  // case, and the raw name for a state this build does not know.
  label: string;
  agentType: string;
  contextMode: string;
  brief: string | null;
  lastMessage: string | null;
  inputTokens: number | null;
  outputTokens: number | null;
  waitingOn: WaitingOn | null;
  handle: string | null;
  child: string | null;
}

// The block content for one spawn point: the live info when claimed, the
// static spawn marker otherwise.
export function crewView(ref: SpawnRef, info: SubagentInfo | null): CrewView {
  if (!info) {
    return {
      label: 'spawned',
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
  const state = info.state;
  const phase: CrewPhase =
    state.type === 'idle'
      ? state.waiting_on === 'user'
        ? 'waiting-user'
        : 'waiting-parent'
      : state.type === 'awaiting'
        ? // Awaiting is the `QUESTION:` marker: the child is asking the user —
      // the same red "needs you" case as an idle child parked on the user.
        'waiting-user'
        : badgeLabel(state);
  return {
    phase,
    label:
      phase === 'waiting-user'
        ? 'waiting on you'
        : phase === 'waiting-parent'
          ? 'waiting on parent'
          : phase === 'running'
            ? 'running'
            : badgeLabel(state),
    agentType: ref.agentType,
    contextMode: ref.contextMode,
    brief: ref.brief,
    lastMessage: info.last_message,
    inputTokens: info.usage?.input_tokens ?? null,
    outputTokens: info.usage?.output_tokens ?? null,
    waitingOn: waitingOnOf(state),
    handle: info.handle,
    child: info.child
  };
}
