// The one ViewEntry → Entry decoder (roadmap C9: replaces the demo
// fixture's toEntry and the card's payload re-parses; ADR-0006 — the GUI
// decodes entry payloads here, nowhere else). Pure over wire data: the
// dev seam (window.__tau) can feed it directly.
import type {
  AnyEntry,
  Entry,
  Event,
  MessageEntry,
  SubagentPayload,
  TaskPayload,
  Usage,
  ViewEntry
} from './protocol';
import { valueLinesOf } from './markdown';

// A sub-agent's message to the parent (and a parent's message to a child)
// conventionally carries a JSON payload after the prose ("hi — {"word":"hi"}").
// Split it out so the GUI can render the payload as kv lines, the way the
// expanded tool call does. Returns null when the text has no parseable
// trailing object.
export function splitJsonPayload(
  text: string
): { prose: string; kv: Array<{ k: string; lines: string[] }> } | null {
  const t = text.trimEnd();
  if (!t.endsWith('}')) return null;
  for (let i = t.length - 1; i >= 0; i--) {
    if (t[i] !== '{') continue;
    let obj: unknown;
    try {
      obj = JSON.parse(t.slice(i));
    } catch {
      continue;
    }
    if (typeof obj !== 'object' || obj === null || Array.isArray(obj)) continue;
    const kv = Object.entries(obj).map(([k, v]) => ({
      k,
      lines: valueLinesOf(v, 1)
    }));
    if (!kv.length) continue;
    return { prose: t.slice(0, i).replace(/[\s—–\-·:]+$/, ''), kv };
  }
  return null;
}

// The newest observation is wrapped in its provenance-group tags
// (<observation-group id=… range=…> … </observation-group>); the card shows
// the content, not the wrapper.
function stripObservationGroup(s: string): string {
  return s
    .replace(/^<observation-group[^>]*>\s*/, '')
    .replace(/\s*<\/observation-group>\s*$/, '')
    .trim();
}

export function decodeEntry(v: ViewEntry): Entry {
  const p = (v.payload && typeof v.payload === 'object' ? v.payload : {}) as Record<string, unknown>;
  switch (v.kind) {
    case 'user': {
      const text = String(p.text ?? '');
      const source = p.source ? String(p.source) : undefined;
      const sk = p.skill as { name?: unknown; location?: unknown } | undefined;
      return {
        id: v.id,
        kind: 'user',
        text,
        source,
        skill: sk ? { name: String(sk.name ?? ''), location: String(sk.location ?? '') } : undefined,
        // The child → parent direction is payload-derived (source); the
        // parent → child case needs the session's parent link, so the
        // card splits that one itself.
        msg: source ? (splitJsonPayload(text) ?? undefined) : undefined
      };
    }
    case 'assistant': {
      const interrupted = Boolean(p.interrupted);
      return {
        id: v.id,
        kind: interrupted ? 'interrupted' : 'message',
        text: String(p.text ?? ''),
        reasoning: p.reasoning ? String(p.reasoning) : undefined,
        calls: Array.isArray(p.calls)
          ? p.calls
              .map((c) => String((c as { call_id?: unknown }).call_id ?? ''))
              .filter(Boolean)
          : undefined,
        usage: 'usage' in p ? (p.usage as Usage | undefined) : undefined
      };
    }
    case 'tool': {
      const a = p.args;
      return {
        id: v.id,
        kind: 'tool',
        name: String(p.name ?? 'tool'),
        // The provider's call id: the two-phase record's join key (the call
        // entry and its result entry share it; the GUI folds them to one card).
        call_id: p.call_id ? String(p.call_id) : undefined,
        args: a && typeof a === 'object' ? (a as Record<string, unknown>) : undefined,
        output: p.output !== undefined ? String(p.output) : undefined,
        status: toolStatus(p.output)
      };
    }
    case 'om': {
      const o = p as {
        active_observations?: string;
        om_thinking?: string;
        om_input?: string;
        om_suggested_response?: string;
        om_model?: string;
      };
      // The record's active_observations is the whole managed suffix; the
      // block shows what this entry added — the newest observation, past
      // its message boundary.
      const all = o.active_observations ?? String(p as unknown as string);
      const m = all.lastIndexOf('--- message boundary (');
      const nl = m >= 0 ? all.indexOf('\n\n', m) : -1;
      const raw = nl > 0 ? all.slice(nl + 2).trim() : all.trim();
      return {
        id: v.id,
        kind: 'om',
        text: stripObservationGroup(raw),
        thinking: o.om_thinking || undefined,
        input: o.om_input || undefined,
        suggestedResponse: o.om_suggested_response || undefined,
        model: o.om_model || undefined
      };
    }
    case 'system':
      return { id: v.id, kind: 'system', text: String(p.note ?? '') };
    case 'spawn-snapshot':
      return { id: v.id, kind: 'spawn-snapshot', text: String(p.log ?? '') };
    case 'subagent':
      return { id: v.id, kind: 'subagent', text: JSON.stringify(p ?? v.id), payload: p as SubagentPayload };
    case 'task':
      return { id: v.id, kind: 'task', text: JSON.stringify(p ?? v.id), payload: p as TaskPayload };
    default:
      return { id: v.id, kind: v.kind, text: JSON.stringify(p ?? v.id) } as AnyEntry;
  }
}

// Sidecar blob (ADR-0005): an oversized payload arrives null with a blob
// pointer; the payload is fetched on demand and substituted before decode.
// A fetch that fails keeps the pointer — one unreadable blob must not sink
// the window read.
export async function resolveBlobs(
  views: ViewEntry[],
  fetchBlob: (v: ViewEntry) => Promise<unknown>
): Promise<ViewEntry[]> {
  return Promise.all(
    views.map(async (v) => {
      if (v.payload !== null || !v.blob) return v;
      try {
        return { ...v, payload: await fetchBlob(v) };
      } catch {
        return v;
      }
    })
  );
}

// One logical entry appears under two id namespaces: streamed under its
// call_id / u- prefix / provider tool_call_id, persisted under the file's
// counter. The namespaces are not comparable, so the streamed slot is
// canonical: a file entry whose twin is streamed (in entries, live mid-turn,
// or a duplicate file copy from the snapshot) hydrates that slot in place
// and the file copy is dropped. A file entry with no twin takes its own id
// and appends in arrival order.

// A delta that lands before its stream_start (a GUI connecting mid-stream):
// buffered per call until the start or end arrives. Capped per connection —
// a stuck stream must not grow it without bound.
export interface PendingDelta {
  text: string;
  reasoning: string;
}

const PENDING_CAP = 64 * 1024;

// The stream cases of applyEvents as pure array updates: the store assigns
// the results back into its $state and keeps the timing side effects
// (TPS clock, the post-turn tail re-read) itself.
export function applyStreamEvent(
  ev: Event,
  entries: Entry[],
  live: MessageEntry[],
  pending: Map<string, PendingDelta>
): { entries: Entry[]; live: MessageEntry[]; fin: Entry | null } {
  switch (ev.type) {
    case 'stream_start': {
      const pd = pending.get(ev.call_id);
      if (pd) pending.delete(ev.call_id);
      const le: MessageEntry = {
        id: ev.call_id,
        kind: 'message',
        text: pd?.text ?? '',
        reasoning: pd?.reasoning ?? ''
      };
      return { entries, live: [...live, le], fin: null };
    }
    case 'stream_delta': {
      const i = live.findIndex((x) => x.id === ev.call_id);
      let nl = live;
      if (i >= 0) {
        const le = live[i];
        nl = live.slice();
        nl[i] = { ...le, text: le.text + ev.text, reasoning: le.reasoning + (ev.reasoning ?? '') };
      } else {
        const pd = pending.get(ev.call_id) ?? { text: '', reasoning: '' };
        if (pd.text.length < PENDING_CAP) {
          pending.set(ev.call_id, { text: pd.text + ev.text, reasoning: pd.reasoning + (ev.reasoning ?? '') });
        }
      }
      return { entries, live: nl, fin: null };
    }
    case 'stream_end': {
      const le = live.find((x) => x.id === ev.call_id);
      const nl = live.filter((x) => x.id !== ev.call_id);
      let fin: Entry | null = null;
      if (le) {
        fin = {
          id: le.id,
          kind: ev.interrupted ? 'interrupted' : 'message',
          text: le.text,
          reasoning: le.reasoning || undefined,
          usage: ev.usage ?? undefined
        };
      } else {
        // Ended before we saw its start: close it out of the buffer.
        const pd = pending.get(ev.call_id);
        if (pd) {
          pending.delete(ev.call_id);
          fin = {
            id: ev.call_id,
            kind: ev.interrupted ? 'interrupted' : 'message',
            text: pd.text,
            reasoning: pd.reasoning || undefined,
            usage: ev.usage ?? undefined
          };
        }
      }
      let ne = entries;
      if (fin) {
        // The paged read can hydrate this entry's file copy during the turn;
        // that copy is canonical, so only push the streamed one when no file
        // twin exists.
        const dup = ne.some((e) => /^\d+$/.test(e.id) && e.kind === fin.kind && e.text === fin.text);
        if (!dup) ne = [...ne, fin];
      }
      return { entries: ne, live: nl, fin };
    }
    default:
      return { entries, live, fin: null };
  }
}

// Tool entries key on the provider's tool_call_id (not the stream call_id):
// that is the id persisted in the file payload, so the hydration twin
// matches, and two tools in one assistant call no longer collapse into one
// card.
export function applyToolEvent(
  ev: Event,
  entries: Entry[],
  live: MessageEntry[]
): { entries: Entry[]; live: MessageEntry[] } {
  if (ev.type === 'tool_start') {
    // A tool call follows the assistant text that requested it: settle the
    // streaming entries into the list first so the card lands after that
    // text, not after the response that follows it.
    const ne = [...entries];
    for (const le of live) {
      ne.push({ id: le.id, kind: 'message', text: le.text, reasoning: le.reasoning || undefined });
    }
    const xi = ne.findIndex((e) => e.id === ev.tool_call_id);
    if (xi >= 0 && ne[xi].kind === 'tool') {
      const e: AnyEntry = ne[xi];
      // The turn-end pump re-emits ToolStart for every tool: a card that
      // already has a live output stays put, no running→ok flicker.
      if (!e.output) ne[xi] = { ...e, status: 'running' };
    } else {
      // The end-of-turn pump can deliver this after a later turn's entries
      // have already landed: place the card right after the assistant call
      // that made it, not at the tail. The id match covers the streamed
      // copy (id = the stream's call_id); once the entry's file twin owns
      // the slot, its id is the file counter (the stream_end dedupe drops
      // the streamed copy against a numeric twin of identical text), so
      // fall back to the call reference the entry carries. The tail append
      // is last resort.
      let ai = ne.findIndex((x) => x.id === ev.call_id);
      if (ai < 0) {
        ai = ne.findIndex(
          (x) => (x.kind === 'message' || x.kind === 'interrupted') && x.calls?.includes(ev.tool_call_id)
        );
      }
      const card: Entry = { id: ev.tool_call_id, kind: 'tool', name: ev.name, status: 'running', call_id: ev.tool_call_id };
      if (ai >= 0) ne.splice(ai + 1, 0, card);
      else ne.push(card);
    }
    return { entries: ne, live: [] };
  }
  if (ev.type !== 'tool_end') return { entries, live };
  const i = entries.findIndex((x) => x.id === ev.tool_call_id);
  if (i < 0 || entries[i].kind !== 'tool') return { entries, live };
  const e: AnyEntry = entries[i];
  const ne = entries.slice();
  ne[i] = { ...e, status: toolStatus(ev.output), output: ev.output === undefined ? undefined : String(ev.output) };
  return { entries: ne, live };
}

// The persisted tool output records a failure: the bash executor prefixes
// "exit N" (non-zero N) or "bash: <error>" (spawn/timeout). All other tools
// report free text — a failure there is content, not status.
function toolStatus(output: unknown): 'running' | 'ok' | 'error' {
  if (output === undefined) return 'running';
  const s = String(output);
  const m = /^exit (\d+)/.exec(s);
  if (m) return m[1] === '0' ? 'ok' : 'error';
  if (/^bash: /.test(s)) return 'error';
  return 'ok';
}

// Streamed ids are non-numeric (call_id, u-…, the provider's tool_call_id);
// a file-counter id is a snapshot copy, never a twin.
function isTwin(e: Entry, v: ViewEntry, next: Entry): boolean {
  if (/^\d+$/.test(e.id)) return false;
  const a: AnyEntry = e;
  const n: AnyEntry = next;
  if (n.kind === 'tool') {
    return e.id === String((v.payload as Record<string, unknown>).call_id ?? '');
  }
  // An empty-text assistant (reasoning-only) has no text to match on: its
  // reasoning is the identity — the streamed copy and the file copy carry
  // byte-identical reasoning.
  if (!n.text) return Boolean(n.reasoning) && a.reasoning === n.reasoning;
  return a.text === n.text;
}

// The merge compares through the escape variant: every variant carries the
// same optional fields, so the change test is one comparison.
function entryChanged(a: Entry, b: Entry): boolean {
  const x: AnyEntry = a;
  const y: AnyEntry = b;
  return (
    x.kind !== y.kind ||
    x.text !== y.text ||
    x.reasoning !== y.reasoning ||
    x.output !== y.output ||
    x.status !== y.status ||
    x.name !== y.name ||
    x.args !== y.args ||
    x.source !== y.source
  );
}

function hydrate(e: Entry, n: Entry): Entry {
  const a: AnyEntry = e;
  const b: AnyEntry = n;
  return {
    ...a,
    kind: b.kind,
    text: b.text,
    reasoning: b.reasoning,
    calls: b.calls,
    output: b.output,
    status: b.status,
    name: b.name,
    args: b.args,
    usage: b.usage,
    source: b.source,
    payload: b.payload,
    msg: b.msg
  };
}

// A tool call is recorded in two phases: the call entry (recorded before
// dispatch, no output) and the result entry (after dispatch, same call_id).
// The file carries both; the GUI shows one card — the call slot keeps its
// position (ordered before the side-effect entries the dispatch appended)
// and adopts the result's output. Pairing is by call_id when both entries
// carry one; entries without a call_id (pre-change files, preview
// placeholders) never blind-pair, so a mixed list degrades to a duplicate
// card that the next read resolving the pair heals — never a mis-pair.
export function collapseToolCalls(entries: Entry[]): Entry[] {
  const toolIdx: number[] = [];
  for (let i = 0; i < entries.length; i++) {
    if ((entries[i] as AnyEntry).kind === 'tool') toolIdx.push(i);
  }
  if (toolIdx.length < 2) return entries;
  const cid = (i: number) => (entries[i] as AnyEntry).call_id ?? undefined;
  const hasOut = (i: number) => {
    const o = (entries[i] as AnyEntry).output;
    return o !== undefined && o !== '';
  };
  // A tool call is recorded in two phases: the call (before dispatch, empty
  // output) and the result (after dispatch, with output). In a batch every
  // call is appended before any result, so a result pairs with its call by
  // call_id -- adjacency in the tool subsequence only holds for one call.
  const callPhase = new Map<string, number>();
  for (const i of toolIdx) {
    const id = cid(i);
    if (id !== undefined && !hasOut(i) && !callPhase.has(id)) callPhase.set(id, i);
  }
  const seenResult = new Set<string>();
  const drop = new Set<number>();
  let outList = entries;
  for (const i of toolIdx) {
    if (drop.has(i) || !hasOut(i)) continue;
    const id = cid(i);
    if (id === undefined) continue;
    const j = callPhase.get(id);
    if (j !== undefined) {
      // Pair found: the call slot adopts the result's output + status; the
      // result slot is dropped. Consuming the id means a later same-call
      // result is a re-read, not a fresh pair.
      outList = outList.slice();
      const c = outList[j] as AnyEntry;
      const r = outList[i] as AnyEntry;
      outList[j] = { ...c, output: r.output, status: r.status };
      drop.add(i);
      callPhase.delete(id);
      seenResult.add(id);
      continue;
    }
    // No call slot for this call_id: a re-read of an earlier result (drop
    // it) or a standalone result with no call phase (keep it).
    if (seenResult.has(id)) drop.add(i);
    else seenResult.add(id);
  }
  if (drop.size === 0) return outList;
  return outList.filter((_, i) => !drop.has(i));
}

export function mergeHydrated(
  entries: Entry[],
  live: MessageEntry[],
  views: ViewEntry[]
): { entries: Entry[]; live: MessageEntry[] } {
  let ne = entries;
  let nl = live;
  // id → index, rebuilt on a structural change (a splice): a per-view id
  // lookup is O(1) instead of an O(n) findIndex, so a burst of window pages
  // no longer costs O(n·window). (The twin scan below is content-based and
  // stays a walk; a structural rebuild is rare.)
  const index = new Map<string, number>();
  for (let k = 0; k < ne.length; k++) index.set(ne[k].id, k);
  for (const v of views) {
    const next = decodeEntry(v);
    const i = index.get(v.id);
    if (i !== undefined) {
      // A file copy is already present (snapshot). If the streamed twin of
      // the same logical entry exists too, drop the streamed slot and keep
      // the file copy: the card moves to where the file says it belongs (its
      // file position), matching a reload. The old "keep the streamed slot"
      // left live tool cards stuck at the tail of the hydrated list.
      const ti = ne.findIndex((e, j) => j !== i && isTwin(e, v, next));
      if (ti >= 0) {
        ne = ne.slice();
        ne.splice(ti, 1);
        // The splice shifted every index after ti: rebuild the id map. The
        // file copy (at i) stays, now at its file position.
        index.clear();
        for (let k = 0; k < ne.length; k++) index.set(ne[k].id, k);
        continue;
      }
      // A call-phase view (empty output) must not blank a slot that has
      // already adopted its pair's output (the two-phase tool record).
      const staleCallPhase =
        next.kind === 'tool' &&
        !(next as AnyEntry).output &&
        Boolean((ne[i] as AnyEntry).output);
      if (!staleCallPhase && entryChanged(ne[i], next)) {
        ne = ne.slice();
        ne[i] = next;
      }
      continue;
    }
    const li = nl.findIndex((e) => isTwin(e, v, next));
    if (li >= 0) {
      // Mid-turn: the live slot adopts the file's payload but keeps its
      // streamed id, so the promoted entry keeps its position. Guarded on
      // a real change — the 25 ms fetch re-enters while the turn runs and
      // an unconditional replace never converges.
      const l = nl[li];
      const n: AnyEntry = next;
      if (l.text !== n.text || (l.reasoning ?? '') !== (n.reasoning ?? '')) {
        nl = nl.map((x, j) => (j === li ? ({ ...(next as MessageEntry), id: l.id } as MessageEntry) : x));
      }
      continue;
    }
    if (ne.some((e) => isTwin(e, v, next))) continue;
    // Insert at the file position (id is a zero-padded counter, so it sorts
    // in file order), not the end: a live entry (a subagent/task/om card)
    // lands mid-file, and appending it would show it after later entries
    // until a reload reorders the list.
    const at = ne.findIndex((e) => e.id > next.id);
    ne =
      at === -1
        ? [...ne, next]
        : [...ne.slice(0, at), next, ...ne.slice(at)];
    index.clear();
    for (let k = 0; k < ne.length; k++) index.set(ne[k].id, k);
  }
  return { entries: collapseToolCalls(ne), live: nl };
}
