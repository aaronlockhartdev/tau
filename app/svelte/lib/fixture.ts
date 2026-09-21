// The demo/fixture data (?demo=1): a stand-in for the Tauri dispatch that
// serves the exact same protocol shapes, so the performance bar and every
// rendering path run with zero network. The 10k-entry generator mirrors
// crates/tau-core/tests/large_session.rs (~200 B chained payloads) with the
// entry mix of the prototype; the markdown edge cases are the prototype's.

import type { Entry, SessionMeta, SkillInfo, ViewEntry } from './protocol';
interface FixtureEntry {
  id: string;
  parent: string | null;
  kind: string;
  timestamp: number;
  payload: unknown;
  first_kept: string | null;
}

export interface DemoSession {
  meta: SessionMeta;
  entries: Entry[]; // metadata skeleton (preview text)
  views: ViewEntry[]; // payloads, for paged reads
}

const N = 10000;
const T0 = Date.now() - N * 47000; // relative to now — the MRU column renders 'Xd' ago, not 8815h

function userPayload(text: string): unknown {
  return { text, lane: 'follow_up' };
}

function assistantPayload(text: string, extra: Partial<Record<string, unknown>> = {}): unknown {
  return {
    text,
    reasoning: '',
    interrupted: false,
    usage: {
      input_tokens: 1200 + (text.length % 900),
      output_tokens: 40 + (text.length % 30),
      total_tokens: 1240
    },
    calls: [],
    ...extra
  };
}

function toolPayload(name: string, arg: string, output: string): unknown {
  return { call_id: 'call-' + arg, name, args: { path: arg }, output };
}

function omPayload(i: number): unknown {
  return {
    frozen_prefix: '',
    active_observations: `• [${i}] group g${Math.floor(i / 11)} — observed 6k tokens into ${
      1 + (i % 5)
    } entries`,
    cursor: { entry_id: String(i).padStart(8, '0'), timestamp: T0 + i * 47000 },
    generation: Math.floor(i / 11),
    observation_tokens: 40000 - (i % 9000),
    pending_tokens: i % 6000,
    prefix_demoted: false
  };
}

function genPayload(i: number): unknown {
  switch (i % 11) {
    case 0:
      return userPayload(`(message ${i}) keep going on the ${i % 2 ? 'provider' : 'protocol'} thread`);
    case 1:
      return assistantPayload(
        i % 3
          ? 'Worked through the next chunk. The refactor held up — **tests green**, no drift in the envelope.'
          : 'Tests green. Moving to the next module; the cursor read is the rebuild path.'
      );
    case 2:
      return toolPayload(
        ['read', 'edit', 'bash', 'read', 'task_evidence', 'bash', 'read', 'edit', 'bash'][i % 9],
        `src/protocol/file${i}.rs`,
        `line ${i} of output…\nline ${i + 1} of output…`
      );
    case 3:
      return omPayload(i);
    case 4:
      return { note: `task t${1 + (i % 5)}: step ${1 + (i % 3)}/3 → active` };
    case 5:
      return assistantPayload('Continuing. ' + (i % 2 ? 'Context stable.' : 'Cache hit on the system prompt.'));
    case 6:
      return toolPayload(
        ['read', 'edit', 'bash', 'read', 'task_evidence', 'bash', 'read', 'edit', 'bash'][(i + 3) % 9],
        `src/${(i * 7) % 40}.rs`,
        '(no output)'
      );
    case 7:
      return userPayload(`(message ${i})`);
    case 8:
      return toolPayload(
        ['read', 'edit', 'bash', 'read', 'task_evidence', 'bash', 'read', 'edit', 'bash'][(i + 5) % 9],
        `docs/adr/000${1 + (i % 6)}*.md`,
        '2 files matched'
      );
    case 9:
      return { note: `compaction: span closed, ${400 + (i % 40)} entries compressed` };
    default:
      return assistantPayload(
        'Note to self: ' + (i % 4 ? 'keep the envelope multiplexed.' : 'the cursor query is the rebuild path.')
      );
  }
}

// The markdown edge cases the prototype verified, as real entries at the
// tail (the side-by-side check against prototype/gui-ia/index.html).
function mk(
  id: number,
  kind: string,
  payload: unknown,
  first_kept: string | null = null
): FixtureEntry {
  return {
    id: String(id).padStart(8, '0'),
    parent: String(id - 1).padStart(8, '0'),
    kind,
    timestamp: T0 + id * 47000,
    payload,
    first_kept
  };
}

function specialEntries(): FixtureEntry[] {
  return [
    mk(9990, 'user', userPayload('Show me the SSE parser edge case you found.')),
    mk(
      9991,
      'assistant',
      assistantPayload(
        '### The `Content-Type` fallback\n' +
          'The llama.cpp shim sends **no** content type on some builds:\n' +
          '```rust\n// this line must stay CODE, not markdown:\nfn detect(t: &str) -> bool { t.contains("**bold**") && t.contains("`code`") }\n```\n' +
          '- `x-ndjson` → treat as json lines\n- *(missing)* → sniff the first line\n\nSee [the shim notes](docs/research/llama-shim.md) for the full list.'
      )
    ),
    mk(
      9992,
      'tool',
      toolPayload(
        'edit',
        'src/protocol/sse.rs  {wUp, k2F} → 3 lines',
        'wUp│- pub struct Command { pub kind: String }\nwUp│+ #[derive(Serialize, Deserialize)]\nwUp│+ pub enum Command { Open(OpenCmd), Send(SendCmd) }\nk2F│ pub struct Envelope { pub ws: WsId, ... }'
      )
    ),
    mk(
      9993,
      'assistant',
      assistantPayload(
        'One more edge: a mid-stream **error** frame must not end the call:\n```\n{"type":"error","message":"provider: overloaded"}\n{"type":"response.output_text.delta","delta":" — partial kept"}\n```\n*The partial survives; usage commits only on `response.completed`.*'
      )
    ),
    mk(9994, 'assistant', assistantPayload('I was cut off here by a force — ', { interrupted: true })),
    mk(9995, 'om', omPayload(9995)),
    mk(9996, 'system', { note: 'stopped after 32 tool rounds without the model ending the turn' }),
    mk(9997, 'spawn-snapshot', {
      parentSession: 'demo-parent',
      range: 'g1–g12',
      log: "frozen prefix: the parent's observation log verbatim (never re-observed, never re-reflected)"
    }),
    mk(9998, 'user', userPayload('Park and wait for the sub-agent benchmark.')),
    mk(
      9999,
      'assistant',
      assistantPayload(
        'Kicked off the snapshot benchmark as a sub-agent:\n```rust\nlet snap = session.snapshot();\nassert!(snap.size_bytes() < 2_000_000);\n```\n**I will park and wait for it.** (ends turn)'
      )
    )
  ];
}

function view(e: FixtureEntry): ViewEntry {
  return {
    id: e.id,
    parent: e.parent,
    kind: e.kind,
    timestamp: e.timestamp,
    payload: e.payload,
    blob: null,
    first_kept: e.first_kept
  };
}

// Map a protocol ViewEntry to the flat card shape the transcript renders.
export function toEntry(v: ViewEntry): Entry {
  const p = v.payload as Record<string, unknown>;
  const usage =
    'usage' in (p as object) ? (p.usage as Entry['usage']) : undefined;
  switch (v.kind) {
    case 'user': {
      const sk = p.skill as { name?: unknown; location?: unknown } | undefined;
      return {
        id: v.id,
        kind: 'user',
        text: String(p.text ?? ''),
        source: p.source ? String(p.source) : undefined,
        skill: sk ? { name: String(sk.name ?? ''), location: String(sk.location ?? '') } : undefined,
        usage
      };
    }
    case 'assistant': {
      const interrupted = Boolean(p.interrupted);
      return {
        id: v.id,
        kind: interrupted ? 'interrupted' : 'message',
        text: String(p.text ?? ''),
        reasoning: p.reasoning ? String(p.reasoning) : undefined,
        usage
      };
    }
    case 'tool': {
      const a = p.args as Record<string, unknown> | undefined;
      return {
        id: v.id,
        kind: 'tool',
        name: String(p.name ?? 'tool'),
        args: a ? JSON.stringify(a) : undefined,
        output: p.output !== undefined ? String(p.output) : undefined,
        status: p.output === undefined ? 'running' : 'ok'
      };
    }
    case 'om': {
      const o = p as { active_observations?: string };
      // The record's active_observations is the whole managed suffix; the
      // block shows what this entry added — the newest observation, past
      // its message boundary.
      const all = o.active_observations ?? String(p as unknown as string);
      const m = all.lastIndexOf('--- message boundary (');
      const nl = m >= 0 ? all.indexOf('\n\n', m) : -1;
      return {
        id: v.id,
        kind: 'om',
        text: nl > 0 ? all.slice(nl + 2).trim() : all.trim(),
        status: 'ok'
      };
    }
    case 'system':
      return { id: v.id, kind: 'system', text: String(p.note ?? ''), status: 'ok' };
    case 'spawn-snapshot':
      return { id: v.id, kind: 'spawn-snapshot', text: String(p.log ?? ''), status: 'ok' };
    default:
      return { id: v.id, kind: v.kind, text: JSON.stringify(p ?? v.id), status: 'ok' };
  }
}

export function buildDemoSession(): DemoSession {
  const raw: FixtureEntry[] = [];
  for (let i = 0; i < N; i++) {
    raw.push({
      id: String(i).padStart(8, '0'),
      parent: i === 0 ? null : String(i - 1).padStart(8, '0'),
      kind: ['user', 'assistant', 'tool', 'om', 'system', 'assistant', 'tool', 'user', 'tool', 'system', 'assistant'][i % 11],
      timestamp: T0 + i * 47000,
      payload: genPayload(i),
      first_kept: null
    });
  }
  // The 10 special entries replace the last 10 generated ones; the chain
  // stays intact (the same parent rule as the generated entries).
  const specials = specialEntries();
  raw[N - 10] = specials[0];
  for (let k = 0; k < 9; k++) raw[N - 10 + 1 + k] = specials[1 + k];

  // Mid-session (index 5000): a /skill: invocation (ticket #28) — the
  // entry records the expansion template and carries the skill marker,
  // the green block's source. Mid, not at the tail: the demo streams
  // keep appending after the 10k fixture.
  const skillEntry = mk(5000, 'user', {
    text:
      'Skill `tauri-app-creator` — follow the instructions below. The skill directory is /home/user/git/tau/.agents/skills/tauri-app-creator; resolve relative paths in the instructions against it.\n\n' +
      'Scaffold a Tauri v2 application with the Svelte frontend: 1) run `npm create tauri-app` choosing the Svelte template, 2) wire the plugin permissions into tauri.conf.json, 3) verify with `cargo build` and the dev server, then hand back the tree layout.\n\n' +
      'User request: set up a new tauri + svelte workspace',
    lane: 'steering',
    skill: {
      name: 'tauri-app-creator',
      location: '/home/user/git/tau/.agents/skills/tauri-app-creator/SKILL.md'
    }
  });
  raw[5000] = skillEntry;

  const views = raw.map(view);
  const entries: Entry[] = views.map((v) => {
    const e = toEntry(v);
    // Skeleton: the card shows the first line until a paged read fills it.
    e.text = (e.text ?? '').split('\n')[0].slice(0, 80);
    return e;
  });

  const meta: SessionMeta = {
    id: 'demo',
    workspace: 'w-demo',
    title: 'Protocol crate: messages & events (10k fixture)',
    parent: null,
    created: T0,
    leaf: raw[raw.length - 1].id,
    model: 'vllm/qwen3.8-27b',
    usage: null
  };

  return { meta, entries, views };
}

// The demo's skill registry (the store's demo-mode stand-in for the
// skill_list command): one catalog skill and one model-invocation-disabled
// skill — the dropdown is that one's only door.
export function demoSkills(): SkillInfo[] {
  return [
    {
      name: 'tauri-app-creator',
      description: 'Scaffold a Tauri v2 app with a Svelte frontend.',
      location: '/home/user/git/tau/.agents/skills/tauri-app-creator/SKILL.md',
      model_invocation: true
    },
    {
      name: 'tauri-app-sql',
      description: 'Wire the SQL plugin: migrations, permissions, queries.',
      location: '/home/user/git/tau/.agents/skills/tauri-app-sql/SKILL.md',
      model_invocation: true
    },
    {
      name: 'nightly-build',
      description: 'Runs the nightly build — user-invoked only.',
      location: '/home/user/git/tau/.agents/skills/nightly-build/SKILL.md',
      model_invocation: false
    }
  ];
}
