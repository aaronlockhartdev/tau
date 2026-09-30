// The one ViewEntry → Entry decoder (roadmap C9: replaces the demo
// fixture's toEntry and the card's payload re-parses; ADR-0006 — the GUI
// decodes entry payloads here, nowhere else). Pure over wire data: the
// dev seam (window.__tau) can feed it directly.
import type {
  AnyEntry,
  Entry,
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
        // The provider's call id: names the tool call (one card per call,
        // upserted in place as it goes call -> result).
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

// ADR-0008: the only live op on the transcript. The event's view is the
// session file's own line, so the decoded entry takes the map slot for its
// id — a first sight creates the card, a re-emission updates it in place;
// the slot's position is the id's (the creation order), so it never moves.
export function upsertEntry(entries: Record<string, Entry>, v: ViewEntry): void {
  entries[v.id] = decodeEntry(v);
}

// The persisted tool output records a failure: the bash executor prefixes
// "exit N" (non-zero N) or "bash: <error>" (spawn/timeout). All other tools
// report free text — a failure there is content, not status.
function toolStatus(output: unknown): 'running' | 'ok' | 'error' {
  if (output === undefined || output === '') return 'running';
  const s = String(output);
  const m = /^exit (\d+)/.exec(s);
  if (m) return m[1] === '0' ? 'ok' : 'error';
  if (/^bash: /.test(s)) return 'error';
  return 'ok';
}

