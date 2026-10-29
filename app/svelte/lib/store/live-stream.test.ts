// @vitest-environment jsdom
// The store's live-stream suite (ADR-0008, #303): the deltas of a run grow
// one card, the entry's line is the record, and the TPS clock reads the
// stream's window (#320).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));
import { openSession } from '../rules/sessions';
import { applyEvents, store } from './store.svelte';
import { USAGE, WS, entryView, freshStore, snap } from '../testing/store-harness';

beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  vi.useRealTimers();
});

describe('applyEvents: streaming (entry_upsert, ADR-0008)', () => {
  function oneSession() {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    return store.sessions['s1']!;
  }
  function upsert(id: string, kind: string, payload: Record<string, unknown>) {
    return {
      type: 'entry_upsert' as const,
      workspace: WS.id,
      session: 's1',
      entry: entryView(id, kind, payload)
    };
  }
  // #303: one frame's growth of a streaming assistant entry.
  function delta(id: string, seq: number, text: string, reasoning = '') {
    return {
      type: 'stream_delta' as const,
      workspace: WS.id,
      session: 's1',
      id,
      seq,
      text,
      reasoning
    };
  }

  // The wire shape the app's production tee sends (harness/launch.rs), read
  // as the protocol declares it: every frame of a run grows the card (#303).
  it('grows a card from every frame of the wire event the app sends', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', id: '00000001', seq: 1, text: 'li', reasoning: '' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', id: '00000001', seq: 2, text: 've', reasoning: '' }
    ]);
    const card = s.entries['00000001']!;
    expect(card.kind).toBe('message');
    if (card.kind === 'message') expect(card.text).toBe('live');
  });

  it('a turn that fails before its first output leaves "starting" (no stuck running badge)', () => {
    const s = oneSession();
    s.turn = 'starting';
    applyEvents([
      { type: 'system', workspace: WS.id, session: 's1', kind: { kind: 'error', message: 'provider 4xx' } }
    ]);
    expect(s.turn).toBe('idle');
  });

  it('streams a turn to completion: upserts grow the card, stream_end idles the turn with usage', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'hel', reasoning: 'thinking', interrupted: false }),
      upsert('00000001', 'assistant', { text: 'hello', reasoning: 'thinking', interrupted: false }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    expect(s.turn).toBe('idle');
    expect(s.usage).toEqual(USAGE);
    const e = s.entries['00000001']!;
    expect(e).toBeTruthy();
    expect(e.kind).toBe('message');
    if (e.kind === 'message' || e.kind === 'interrupted') {
      expect(e.text).toBe('hello');
      expect(e.reasoning).toBe('thinking');
    }
  });

  it('a card renders on first sight; a re-emission updates it in place, never repositioning', () => {
    const s = oneSession();
    applyEvents([
      upsert('00000001', 'user', { text: 'hi' }),
      upsert('00000002', 'assistant', { text: 'a', interrupted: false }),
      upsert('00000001', 'user', { text: 'hi' })
    ]);
    expect(Object.keys(s.entries)).toEqual(['00000001', '00000002']);
    expect(s.entries['00000001']?.kind).toBe('user');
  });

  it('marks an interrupted response as such (the payload flag decides the kind)', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'ab', interrupted: true }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: true, usage: null }
    ]);
    expect(s.entries['00000001']?.kind).toBe('interrupted');
  });

  it('prefers the server duration over the wall clock for TPS', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    await vi.advanceTimersByTimeAsync(2000);
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE, duration_secs: 1 }
    ]);
    // 10 output tokens over the server's 1 s — not 5 over the 2 s wall clock.
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('measures wall-clock TPS from the first delta, excluding prefill', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    // A 2 s prefill gap before the first token: it must not count.
    await vi.advanceTimersByTimeAsync(2000);
    applyEvents([upsert('00000001', 'assistant', { text: 'a', interrupted: false })]);
    await vi.advanceTimersByTimeAsync(1000);
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    // 10 tokens over the 1 s after the first delta — not 3.33 over the 3 s
    // wall clock that a stream-start base would give.
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('reports no TPS when the stream ends before any token', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    await vi.advanceTimersByTimeAsync(1000);
    // Usage arrived but no assistant delta ever did: no first-delta clock,
    // so no TPS (and no divide by zero).
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    expect(s.tps).toBe(0);
    vi.useRealTimers();
  });

  it('runs the live TPS estimate from the first delta', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    // 2 s prefill, then the first character opens the estimate's clock.
    await vi.advanceTimersByTimeAsync(2000);
    applyEvents([delta('00000001', 1, 'a')]);
    await vi.advanceTimersByTimeAsync(2000);
    // ~2 s after the first delta, 80 chars accumulated ≈ 20 tokens → 10 tps.
    applyEvents([delta('00000001', 2, 'x'.repeat(79))]);
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('the deltas of a stream grow one card in seq order', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      delta('00000001', 1, 'hel', 'think'),
      delta('00000001', 2, 'lo'),
      delta('00000001', 3, ' world')
    ]);
    const e = s.entries['00000001']!;
    expect(e.kind).toBe('message');
    if (e.kind === 'message') {
      expect(e.text).toBe('hello world');
      expect(e.reasoning).toBe('think');
    }
  });

  it('the committed entry replaces the card the deltas built, and carries the usage', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      delta('00000001', 1, 'hel'),
      delta('00000001', 2, 'lo'),
      upsert('00000001', 'assistant', { text: 'hello world', interrupted: false, usage: USAGE }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE, duration_secs: 1 }
    ]);
    const e = s.entries['00000001']!;
    if (e.kind === 'message') {
      expect(e.text).toBe('hello world');
      expect(e.usage).toEqual(USAGE);
    }
  });

  it('a delta out of order is refused, and the file line recovers the text', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      delta('00000001', 1, 'one '),
      delta('00000001', 3, 'three ')
    ]);
    const partial = s.entries['00000001']!;
    if (partial.kind === 'message') expect(partial.text).toBe('one ');
    // The dropped frame is not guessed at; the entry's line is the recovery.
    applyEvents([upsert('00000001', 'assistant', { text: 'one two three', interrupted: false })]);
    const e = s.entries['00000001']!;
    if (e.kind === 'message') expect(e.text).toBe('one two three');
  });

  it('a delta for an unknown session changes nothing', () => {
    oneSession();
    applyEvents([{ ...delta('00000001', 1, 'orphan'), session: 's9' }]);
    expect(store.sessions['s9']).toBeUndefined();
  });

  it('a tool call is one card: the call upsert starts it, the result upsert fills the output', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'run ls', interrupted: false }),
      upsert('00000002', 'tool', { call_id: 't1', name: 'bash', args: { command: 'ls' } }),
      upsert('00000002', 'tool', { call_id: 't1', name: 'bash', args: { command: 'ls' }, output: 'exit 0\n--- stdout ---\nok' }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: null }
    ]);
    const tools = Object.values(s.entries).filter((e) => e.kind === 'tool');
    expect(tools).toHaveLength(1);
    const tool = tools[0]!;
    if (tool.kind === 'tool') {
      expect(tool.name).toBe('bash');
      expect(tool.status).toBe('ok');
      expect(tool.output).toBe('exit 0\n--- stdout ---\nok');
    }
  });

  it('keeps two tools of one call as two cards (one id each)', () => {
    const s = oneSession();
    applyEvents([
      upsert('00000001', 'tool', { call_id: 't1', name: 'a', output: 1 }),
      upsert('00000002', 'tool', { call_id: 't2', name: 'b', output: 2 })
    ]);
    const tools = Object.values(s.entries).filter((e) => e.kind === 'tool');
    expect(tools).toHaveLength(2);
  });
});
