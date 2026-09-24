// The session-tree registry (roadmap C10): the store's registry invariants
// over the session map — stub materialization, the list-merge rule (the
// list is the archive flag's authority), the sub-agent mirror's self-heal
// on open, and archive convergence. ADR-0006's "stateless renderer,
// rebuildable from snapshot + events" is the property these rules
// formalize; the store keeps state and IPC, the module owns the rules.
import type {
  Entry,
  MessageEntry,
  QueuedItem,
  SessionMeta,
  Snapshot,
  SubagentInfo,
  Task,
  Usage
} from './protocol';

export interface PendingMsg {
  text: string;
  lane: 'force' | 'steering' | 'follow-up';
}

export interface SessionState {
  meta: SessionMeta;
  entries: Entry[];
  live: MessageEntry[];
  usage: Usage | null;
  // Output tokens/second of the session's most recent turn (status bar).
  tps: number;
  turn: 'running' | 'idle';
  // The session's OM state (the status bar's gauge): the activity kind
  // (the om_status events) and the gauge values (the snapshots).
  om: {
    kind: 'idle' | 'observing' | 'reflecting';
    observation_tokens: number;
    pending_tokens: number;
    reflector_threshold: number;
  };
  pending: PendingMsg[];
  // The session-tree row (ticket #26): a child session's parent link,
  // its lifecycle state, its archive flag, and its sort key.
  parent: string | null;
  state: 'running' | 'idle' | 'done' | 'failed' | 'stopped';
  // The child's declared wait (parent | user | subagent) — from the state
  // event's detail; annotated on the row and the child's own header.
  waiting_on: string | null;
  archived: boolean;
  mru: number;
  // The session's children (the parent's view — the sub-agent panel) and
  // its tasks (the tasks panel; active ones carry their resume contract).
  subagents: SubagentInfo[];
  tasks: Task[];
}

export type SessionMap = Record<string, SessionState>;

const OM_IDLE = { kind: 'idle', observation_tokens: 0, pending_tokens: 0, reflector_threshold: 0 } as const;

export function laneOf(l: QueuedItem['lane']): PendingMsg['lane'] {
  return l === 'follow_up' ? 'follow-up' : l;
}

function snapshotToState(snap: Snapshot): SessionState {
  const meta = snap.session;
  return {
    meta,
    // metadata skeleton: the card shows the preview until a paged read
    // replaces it with the payload.
    entries: snap.entries.map((m) => ({
      id: m.id,
      kind:
        m.kind === 'assistant'
          ? m.status === 'interrupted'
            ? 'interrupted'
            : 'message'
          : m.kind,
      text: m.preview
    })),
    live: [],
    usage: meta.usage,
    turn: snap.live.turn === 'running' ? 'running' : 'idle',
    tps: 0,
    // The snapshot carries the gauge values; the activity kind is
    // event-driven (an open session is idle until a run starts).
    om: { ...snap.om, kind: 'idle' },
    pending: snap.live.queue.map((q) => ({ text: q.text, lane: laneOf(q.lane) })),
    parent: null,
    state: snap.live.turn === 'running' ? 'running' : 'idle',
    waiting_on: null,
    archived: meta.archived,
    mru: meta.created,
    subagents: snap.live.subagents,
    tasks: snap.live.tasks
  };
}

// A row without a snapshot: the tree shows it, its entries hydrate lazily
// on open. A stub's usage stays null until a real snapshot lands.
export function makeStub(
  meta: SessionMeta,
  parent: string | null,
  state: SessionState['state'],
  waitingOn: string | null,
  mru: number
): SessionState {
  return {
    meta,
    entries: [],
    live: [],
    usage: null,
    tps: 0,
    turn: state === 'running' ? 'running' : 'idle',
    om: { ...OM_IDLE },
    pending: [],
    parent,
    state,
    waiting_on: state === 'idle' ? waitingOn : null,
    archived: meta.archived,
    mru,
    subagents: [],
    tasks: []
  };
}

// Sub-agent mirror sync (a spawn/state event, the open's self-heal): the
// supervisor is ground truth for the child's state and declared wait. A
// corrected row keeps its mru (stable sort) and a real title — a late spawn
// title only fills a stub's blank name.
export function touchChild(
  sessions: SessionMap,
  parentSid: string,
  childSid: string,
  state: SessionState['state'],
  mru: number,
  waitingOn: string | null = null,
  title: string | null = null
): SessionMap {
  const parent = sessions[parentSid];
  const ws = parent?.meta.workspace ?? '';
  const prev = sessions[childSid];
  if (prev) {
    const next: SessionState = { ...prev, state, mru };
    if (waitingOn !== null) next.waiting_on = waitingOn;
    if (title !== null && prev.meta.title === null) next.meta = { ...prev.meta, title };
    return { ...sessions, [childSid]: next };
  }
  const meta: SessionMeta = {
    id: childSid,
    workspace: ws,
    title,
    parent: parentSid,
    created: mru,
    leaf: null,
    model: null,
    usage: null,
    archived: false
  };
  return { ...sessions, [childSid]: makeStub(meta, parentSid, state, waitingOn, mru) };
}

// Merge a session_list into the map: an existing row adopts the list's
// archive flag (the list is the flag's authority — a row archived while
// listed, or listed after a restart, converges on the file's state); a new
// id materializes as a stub (entries hydrate lazily on open).
export function applySessionList(sessions: SessionMap, list: SessionMeta[]): SessionMap {
  let out = sessions;
  for (const m of list) {
    const existing = out[m.id];
    if (existing) {
      if (existing.archived !== m.archived) out = { ...out, [m.id]: { ...existing, archived: m.archived } };
    } else {
      out = { ...out, [m.id]: makeStub(m, m.parent ?? null, 'idle', null, m.created) };
    }
  }
  return out;
}

// Archive (ADR-0005): the row keeps its state (a message resumes it); the
// meta and the flag converge on the command's authority.
export function setArchived(sessions: SessionMap, sid: string, meta: SessionMeta): SessionMap {
  const s = sessions[sid];
  if (!s) return sessions;
  return { ...sessions, [sid]: { ...s, meta, archived: meta.archived } };
}

// Session-open convergence: the snapshot replaces the stub/previous state;
// a child's parent link is not in its own snapshot (it lives in the
// parent's sub-agent list) so it survives re-opens, and the archive flag
// stays with the row (the list is its authority).
export function openSession(sessions: SessionMap, sid: string, snap: Snapshot): SessionMap {
  const next = snapshotToState(snap);
  const prev = sessions[sid];
  if (prev) {
    next.parent = prev.parent;
    next.state = prev.state;
    next.archived = prev.archived;
  }
  let out = { ...sessions, [sid]: next };
  // Self-heal the child stubs: the snapshot's sub-agent list is the
  // supervisor's ground truth, so it both fills a lost spawn and
  // corrects a stale stub (a done child must not keep its pre-wake
  // state). A corrected stub keeps its own mru, so the sort is stable.
  for (const sub of next.subagents) {
    const existing = out[sub.child];
    if (!existing) {
      out = touchChild(out, sid, sub.child, sub.state, Date.now(), sub.waiting_on, null);
      continue;
    }
    out = touchChild(out, sid, sub.child, sub.state, existing.mru, sub.waiting_on, null);
    if (sub.waiting_on === null && sub.state !== 'idle') {
      const c = out[sub.child];
      out = { ...out, [sub.child]: { ...c, waiting_on: null } };
    }
  }
  // A disk-restored child has no live supervisor listing it: synthesize
  // the tab entry from the child stub so the sub-agents tab is never
  // empty; a real spawn/state event (matched by child) replaces it.
  const synthesized = [...next.subagents];
  for (const child of Object.values(out)) {
    if (child.parent !== sid || synthesized.some((x) => x.child === child.meta.id)) continue;
    synthesized.push({
      handle: child.meta.id,
      child: child.meta.id,
      agent_type: 'general',
      context_mode: 'fresh',
      state: child.state,
      waiting_on: child.waiting_on ?? null,
      last_message: null,
      usage: child.usage,
      task: null,
      resume_contract: null
    });
  }
  return { ...out, [sid]: { ...next, subagents: synthesized } };
}

// The group-open rule of the session tree (the left pane): an explicit
// toggle set (openGroups) wins; on the default view a group opens when
// it contains the active session's chain or a running sub-agent child
// (a spawned sub-agent is invisible in a collapsed group — dogfood
// 2026-09-24).
export function groupIsOpen(
  q: { openGroups: string[] | null },
  session: SessionState,
  active: SessionState | null,
  sessions: SessionState[]
): boolean {
  if (q.openGroups !== null) return q.openGroups.includes(session.meta.id);
  if (session.subagents.some((s) => s.state === 'running')) return true;
  let a: SessionState | null = active;
  while (a) {
    if (a.meta.id === session.meta.id) return true;
    const pid = a.parent;
    a = pid ? (sessions.find((s) => s.meta.id === pid) ?? null) : null;
  }
  return false;
}
