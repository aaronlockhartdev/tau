// The demo entry's fixture data: a stand-in for the Tauri dispatch that
// serves the exact same protocol shapes, so the performance bar and every
// rendering path run with zero network. The 10k-entry generator mirrors
// crates/tau-core/tests/large_session.rs (~200 B chained payloads) with the
// entry mix of the prototype; the markdown edge cases are the prototype's.

import type { Entry, SessionMeta, SkillInfo, SubagentInfo, Task, Usage, ViewEntry } from './protocol';
import { decodeEntry } from './entries';
interface FixtureEntry {
  id: string;
  parent: string | null;
  kind: string;
  timestamp: number;
  payload: unknown;
  first_kept: string | null;
}

interface DemoSession {
  meta: SessionMeta;
  entries: Entry[]; // metadata skeleton (preview text)
  views: ViewEntry[]; // payloads, for paged reads
}
// A store SessionState row, without importing the store (this module is
// the store's data source; the demo entry seeds boot's session stubs
// from these, so the shape is structural).
interface FixturePendingMsg {
  text: string;
  lane: 'force' | 'steering' | 'follow-up';
}

interface FixtureSessionState {
  meta: SessionMeta;
  entries: Entry[];
  live: Entry[];
  usage: Usage | null;
  tps: number;
  turn: 'running' | 'idle';
  pending: FixturePendingMsg[];
  parent: string | null;
  state: 'running' | 'idle' | 'done' | 'failed' | 'stopped';
  waiting_on: string | null;
  archived: boolean;
  mru: number;
  subagents: SubagentInfo[];
  tasks: Task[];
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
    const e = decodeEntry(v);
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
    model: 'qwen3.8-27b',
    usage: null,
    archived: false
  };

  return { meta, entries, views };
}

// The demo's skill registry (its stand-in for the skill_list command):
// two catalog skills and one model-invocation-disabled skill — the
// dropdown is that one's only door.
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

// The demo's pane dataset (the prototype's): six sub-agent sessions under
// the fixture session (TWO running), a nested pair under the idle one as
// real store sessions, two archived top-level sessions, and the five
// tasks with steps/criteria/evidence.
function demoMeta(id: string, title: string, created: number, parent: string | null = null): SessionMeta {
  return {
    id,
    workspace: 'w-demo',
    title,
    parent,
    created,
    leaf: null,
    model: 'qwen3.8-27b',
    usage: null,
    archived: false
  };
}

export function demoChild(
  id: string,
  title: string,
  handle: string | null,
  parent: string | null,
  state: SubagentInfo['state'],
  waitingOn: SubagentInfo['waiting_on'],
  lastMessage: string | null,
  usage: Usage,
  created: number,
  mru: number,
  subagents: SubagentInfo[],
  tasks: Task[],
  archived = false
): FixtureSessionState {
  return {
    meta: demoMeta(id, title, created, parent),
    entries: [],
    live: [],
    usage,
    tps: 0,
    turn: state === 'running' ? 'running' : 'idle',
    pending: [],
    parent,
    state,
    waiting_on: state === 'idle' ? waitingOn : null,
    archived,
    mru,
    subagents,
    tasks
  };
}

export function demoChildren(): FixtureSessionState[] {
  const now = Date.now();
  return [
    demoChild('c1', 'provider hardening', 'a', 'demo', 'running', null, 'retry loop done, writing backoff tests', { input_tokens: 41200, output_tokens: 8900, total_tokens: 50100, cached_prompt_tokens: 0 }, now - 3600e3, now - 60e3, [], [demoTask('t1', 'in_progress', 'c1', 0)]),
    demoChild('c2', 'protocol surface', 'b', 'demo', 'idle', 'parent', 'types drafted — needs review sign-off', { input_tokens: 88000, output_tokens: 31000, total_tokens: 119000, cached_prompt_tokens: 0 }, now - 7200e3, now - 180e3, [
      demoSub('g1', 'cg1', 'fresh', 'idle', 'subagent', 'waiting on the field renames'),
      demoSub('g2', 'cg2', 'compacted', 'running', null, 'renaming the event groups')
    ], [demoTask('t4', 'blocked', 'c2', 1)]),
    demoChild('c3', 'config loader', 'c', 'demo', 'done', null, 'loader passes all tests', { input_tokens: 12400, output_tokens: 6100, total_tokens: 18500, cached_prompt_tokens: 0 }, now - 14400e3, now - 3600e3, [], [demoTask('t2', 'done', 'c3', 0)]),
    demoChild('c4', 'fixture generator', 'd', 'demo', 'failed', null, 'provider rejected the degenerate observation run', { input_tokens: 30100, output_tokens: 9400, total_tokens: 39500, cached_prompt_tokens: 0 }, now - 21600e3, now - 5400e3, [], [demoTask('t5', 'done', 'c4', 0)]),
    demoChild('c5', 'doc sweep', 'e', 'demo', 'stopped', null, 'stopped by user mid-sweep', { input_tokens: 5200, output_tokens: 2100, total_tokens: 7300, cached_prompt_tokens: 0 }, now - 28800e3, now - 7200e3, [], []),
    // The prototype's 6th child — TWO running, so the badge rule is testable.
    demoChild('c6', 'snapshot benchmark', 'f', 'demo', 'running', null, 'measuring the 10k snapshot size', { input_tokens: 15600, output_tokens: 3400, total_tokens: 19000, cached_prompt_tokens: 0 }, now - 1800e3, now - 30e3, [], []),
    // The nested pair as real store sessions (depth 2) — double-click opens them.
    demoChild('cg1', 'event renames', 'g1', 'c2', 'idle', 'subagent', 'waiting on the field renames', { input_tokens: 1200, output_tokens: 400, total_tokens: 1600, cached_prompt_tokens: 0 }, now - 3600e3, now - 120e3, [], []),
    demoChild('cg2', 'event groups', 'g2', 'c2', 'running', null, 'renaming the event groups', { input_tokens: 2100, output_tokens: 900, total_tokens: 3000, cached_prompt_tokens: 0 }, now - 3000e3, now - 90e3, [], []),
    // Two archived top-level sessions (the prototype's a1/a2).
    demoChild('a1', 'old: provider spike', null, null, 'done', null, 'archived after the spike closed', { input_tokens: 9800, output_tokens: 2400, total_tokens: 12200, cached_prompt_tokens: 0 }, now - 86400e3 * 30, now - 86400e3 * 5, [], [], true),
    demoChild('a2', 'old: first session store', null, null, 'done', null, 'archived — superseded by the JSONL store', { input_tokens: 14300, output_tokens: 5100, total_tokens: 19400, cached_prompt_tokens: 0 }, now - 86400e3 * 45, now - 86400e3 * 10, [], [], true)
  ];
}

function demoSub(handle: string, child: string, mode: SubagentInfo['context_mode'], state: SubagentInfo['state'], waitingOn: SubagentInfo['waiting_on'], lastMessage: string | null): SubagentInfo {
  return {
    handle,
    child,
    agent_type: 'general',
    context_mode: mode,
    state,
    waiting_on: waitingOn,
    last_message: lastMessage,
    usage: { input_tokens: 1200, output_tokens: 400, total_tokens: 1600, cached_prompt_tokens: 0 },
    task: null,
    resume_contract: null
  };
}

export function demoSubagents(): SubagentInfo[] {
  return [
    demoSub('a', 'c1', 'compacted', 'running', null, 'writing backoff tests'),
    demoSub('b', 'c2', 'fork', 'idle', 'parent', 'types drafted — needs review sign-off'),
    demoSub('c', 'c3', 'fresh', 'done', null, 'loader passes all tests'),
    demoSub('d', 'c4', 'compacted', 'failed', null, 'provider rejected the degenerate run'),
    demoSub('e', 'c5', 'fresh', 'stopped', null, 'stopped by user mid-sweep'),
    demoSub('f', 'c6', 'compacted', 'running', null, 'measuring the 10k snapshot size')
  ];
}

function demoTask(id: string, status: Task['status'], worker: string, skip: number): Task {
  const now = Date.now();
  const base: Task = {
    id,
    title: { t1: 'Harden provider retry/backoff', t2: 'Config loader with layering', t3: 'Migrate tests to the 62-char alphabet', t4: 'Protocol surface: types & events', t5: '10k-entry fixture generator' }[id] ?? id,
    status,
    steps: [],
    criteria: [],
    evidence: [],
    blockers: [],
    notes: [],
    worker: { session: worker, status },
    created_in: 'demo',
    updated: now - 60e3 * (skip + 1),
    resume_contract: undefined,
    decisions: []
  };
  return base;
}

export function demoTasks(): Task[] {
  const now = Date.now();
  const t1 = demoTask('t1', 'in_progress', 'c1', 0);
  t1.steps = [
    { text: 'inventory the retry paths', status: 'done', expected_output: 'retry-path list in the session' },
    { text: 'add backoff + jitter tests', status: 'active', expected_output: 'hardened retry/backoff tests' },
    { text: 'run the live acceptance', status: 'pending', expected_output: 'green live run log' }
  ];
  t1.criteria = [
    { text: 'retries on mid-stream provider errors', status: 'satisfied' },
    { text: 'backoff is capped', status: 'satisfied' },
    { text: 'live run passes', status: 'pending' }
  ];
  t1.evidence = [
    { criterion: 'retries on mid-stream provider errors', summary: 'mid-stream error preserves partial output', command: 'cargo test -p tau-core sse', passed: true },
    { criterion: 'backoff is capped', summary: 'backoff capped at 64 s', command: 'cargo test -p tau-core backoff', passed: true }
  ];
  t1.resume_contract = {
    task: 't1',
    title: t1.title,
    status: 'in_progress',
    current_step: { text: t1.steps[1].text, expected_output: t1.steps[1].expected_output },
    steps: t1.steps,
    evidence: t1.evidence,
    gaps: ['timeout path untested'],
    blockers: [],
    next_action: 'finish the backoff test, then the live acceptance'
  };
  const t2 = demoTask('t2', 'done', 'c3', 1);
  t2.steps = [
    { text: 'parse the TOML layers', status: 'done', expected_output: 'layering rules' },
    { text: 'reject unknown keys', status: 'done', expected_output: 'unknown-key test' }
  ];
  t2.criteria = [
    { text: 'project layer wins on collision', status: 'satisfied' },
    { text: 'unknown keys rejected', status: 'satisfied' }
  ];
  t2.evidence = [{ criterion: 'project layer wins on collision', summary: 'collision + unknown-key tests green', command: 'cargo test -p tau-core config', passed: true }];
  t2.decisions = [{ question: 'wholesale or field-level layering?', decision: 'field-level fallback per entry', decided_by: 'user' }];
  const t3 = demoTask('t3', 'pending', 'c1', 2);
  t3.steps = [
    { text: 'map base36 call sites', status: 'pending', expected_output: 'call-site list' },
    { text: 'port to the 62-char alphabet', status: 'pending', expected_output: 'ported + retested' }
  ];
  t3.criteria = [{ text: 'all tests green on the new alphabet', status: 'pending' }];
  t3.worker = undefined;
  const t4 = demoTask('t4', 'blocked', 'c2', 3);
  t4.steps = [
    { text: 'mirror the Rust types', status: 'done', expected_output: 'protocol.ts surface' },
    { text: 'wire the event groups', status: 'active', expected_output: 'subagent + task event cases' }
  ];
  t4.criteria = [
    { text: 'type-mirrors the crate field-for-field', status: 'satisfied' },
    { text: 'review sign-off', status: 'pending' }
  ];
  t4.evidence = [{ criterion: 'type-mirrors the crate field-for-field', summary: 'svelte-check green against the crate', passed: true }];
  t4.blockers = [{ reason: 'protocol types rejected in review', needs: 'resubmit after the N1 note' }];
  t4.resume_contract = {
    task: 't4',
    title: t4.title,
    status: 'blocked',
    current_step: { text: t4.steps[1].text, expected_output: t4.steps[1].expected_output },
    steps: t4.steps,
    evidence: t4.evidence,
    gaps: ['SubagentInfo shape pending review'],
    blockers: t4.blockers,
    next_action: 'address the review note, resubmit the types'
  };
  const t5 = demoTask('t5', 'done', 'c4', 4);
  t5.steps = [{ text: 'generate the 10k-entry fixture', status: 'done', expected_output: 'the committed fixture' }];
  t5.criteria = [{ text: 'snapshot stays under 2 MB', status: 'satisfied' }];
  t5.evidence = [{ criterion: 'snapshot stays under 2 MB', summary: 'measured 1.9 MB at 10k entries', command: 'cargo test -p tau-protocol snapshot', passed: true }];
  return [t1, t2, t3, t4, t5];
}
