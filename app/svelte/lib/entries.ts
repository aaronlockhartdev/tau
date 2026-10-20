// The one ViewEntry → Entry decoder (roadmap C9: replaces the demo
// fixture's toEntry and the card's payload re-parses; ADR-0006 — the GUI
// decodes entry payloads here, nowhere else). Pure over wire data: the
// dev seam (window.__tau) can feed it directly.
import type {
  Entry,
  SubagentPayload,
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
      const text = typeof p.text === 'string' ? p.text : '';
      const source = typeof p.source === 'string' ? p.source : undefined;
      // The lane rides the payload (the queue-delivery divider's source).
      const lane =
        p.lane === 'force' || p.lane === 'steering' || p.lane === 'follow_up' ? p.lane : undefined;
      const sk = p.skill as { name?: unknown; location?: unknown } | undefined;
      return {
        id: v.id,
        kind: 'user',
        text,
        source,
        lane,
        skill: sk ? { name: typeof sk.name === 'string' ? sk.name : '', location: typeof sk.location === 'string' ? sk.location : '' } : undefined,
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
        text: typeof p.text === 'string' ? p.text : '',
        reasoning: typeof p.reasoning === 'string' ? p.reasoning : undefined,
        calls: Array.isArray(p.calls)
          ? p.calls
            .map((c) => {
              const cid = (c as { call_id?: unknown }).call_id;
              return typeof cid === 'string' ? cid : '';
            })
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
        name: typeof p.name === 'string' ? p.name : 'tool',
        // The provider's call id: names the tool call (one card per call,
        // upserted in place as it goes call -> result).
        call_id: typeof p.call_id === 'string' ? p.call_id : undefined,
        args: a && typeof a === 'object' ? (a as Record<string, unknown>) : undefined,
        output: typeof p.output === 'string' ? p.output : undefined,
        status: decodeToolStatus(p),
        // The sidecar's bytes are gone (blob deleted from disk): the record is
        // what it is — say so, never fake a still-running call.
        unavailable: v.payload == null && v.blob != null ? true : undefined
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
      const all = o.active_observations ?? '';
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
      return { id: v.id, kind: 'system', text: typeof p.note === 'string' ? p.note : '' };
    case 'spawn-snapshot':
      return { id: v.id, kind: 'spawn-snapshot', text: typeof p.log === 'string' ? p.log : '' };
    case 'subagent':
      return { id: v.id, kind: 'subagent', text: JSON.stringify(p ?? v.id), payload: p as SubagentPayload };
    default:
      return { id: v.id, kind: v.kind, text: JSON.stringify(p ?? v.id) };
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

// ADR-0008: the only live op on the transcript. The event's view is the
// session file's own line, so the decoded entry takes the map slot for its
// id — a first sight creates the card, a re-emission updates it in place;
// the slot's position is the id's (the creation order), so it never moves.
export function upsertEntry(entries: Record<string, Entry>, v: ViewEntry): void {
  entries[v.id] = decodeEntry(v);
}

// The persisted tool output records a failure: the bash executor prefixes
// "exit N" (non-zero N) or "bash: <error>" (spawn/timeout). Every other tool
// prefixes a failure with its own name ("read: cannot read ...") and the
// dispatcher prefixes routing failures; a success never does, so the prefix
// is a reliable failure discriminator.
export function toolStatus(name: string, output: unknown): 'running' | 'ok' | 'error' {
  if (output === undefined || output === '') return 'running';
  const s = typeof output === 'string' ? output : '';
  if (name === 'bash') {
    const m = /^exit (\d+)/.exec(s);
    if (m) return m[1] === '0' ? 'ok' : 'error';
    if (/^bash: /.test(s)) return 'error';
    // A completed bash result always leads with `exit N` (success or non-zero)
    // or `bash: ` (spawn/timeout/stop). A non-empty output with neither prefix
    // is in-flight streaming (#197): the command is still running, not done.
    return 'running';
  }
  if (s.startsWith(`${name}: `)) return 'error';
  if (/^(error:|unknown tool:)/.test(s)) return 'error';
  return 'ok';
}

// The core's explicit status (B5) wins when present; the prefix heuristic is
// the back-compat fallback for records written before it existed. A running
// call has neither an explicit status nor an output, so it falls through to
// toolStatus's 'running'.
function decodeToolStatus(p: Record<string, unknown>): 'ok' | 'running' | 'error' {
  const s = p.status;
  if (s === 'ok' || s === 'error') return s;
  return toolStatus(typeof p.name === 'string' ? p.name : '', p.output);
}

