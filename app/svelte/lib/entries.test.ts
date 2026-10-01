import { describe, expect, it } from 'vitest';
import { decodeEntry, resolveBlobs, splitJsonPayload, upsertEntry } from './entries';
import type { Entry, ViewEntry } from './protocol';

// The om entry carries the newest observation (its provenance-group wrapper
// stripped) plus the run's display details for the observation card.
const payload = {
  active_observations:
    '\n--- message boundary (123) ---\n\n' +
    '<observation-group id="abc" range="1:5">\n' +
    'Date: Sep 25, 2026\n* 🔴 (16:21) user set up a workbench\n' +
    '</observation-group>',
  om_thinking: 'deciding what to keep',
  om_input: 'the transcript',
  om_suggested_response: 'walk through it',
  om_model: 'dev/qwen3.8-27b'
};

function omView(): ViewEntry {
  return {
    id: '5',
    parent: null,
    kind: 'om',
    timestamp: 123,
    payload,
    blob: null,
    first_kept: null
  };
}

describe('upsertEntry (ADR-0008)', () => {
  it('a first sight creates the card under its id', () => {
    const entries: Record<string, Entry> = {};
    upsertEntry(entries, omView());
    expect(Object.keys(entries)).toEqual(['5']);
    expect(entries['5'].kind).toBe('om');
  });

  it('a re-emission updates the slot in place and never repositions it', () => {
    const v = (id: string, text: string): ViewEntry => ({
      id,
      parent: null,
      kind: 'assistant',
      timestamp: 1,
      payload: { text, reasoning: 'r' },
      blob: null,
      first_kept: null
    });
    const entries: Record<string, Entry> = {};
    upsertEntry(entries, v('00000001', 'hel'));
    upsertEntry(entries, v('00000002', 'other'));
    upsertEntry(entries, v('00000001', 'hello'));
    expect(Object.keys(entries)).toEqual(['00000001', '00000002']);
    const e = entries['00000001'];
    if (e.kind === 'message' || e.kind === 'interrupted') expect(e.text).toBe('hello');
  });
});
describe('decodeEntry (tool)', () => {
  const toolView = (id: string, output: string): ViewEntry => ({
    id,
    parent: null,
    kind: 'tool',
    timestamp: 1,
    payload: { call_id: 'c1', name: 'bash', args: {}, output },
    blob: null,
    first_kept: null
  });

  it('a call-phase tool (empty output) decodes to running, not ok', () => {
    const entries: Record<string, Entry> = {};
    upsertEntry(entries, toolView('3', ''));
    const e = entries['3'];
    expect(e.kind).toBe('tool');
    if (e.kind === 'tool') expect(e.status).toBe('running');
  });

  it('a result-phase tool (non-empty output) decodes to ok', () => {
    const entries: Record<string, Entry> = {};
    upsertEntry(entries, toolView('4', 'done'));
    const e = entries['4'];
    if (e.kind === 'tool') expect(e.status).toBe('ok');
  });
});

describe('decodeEntry (om)', () => {
  it('strips the observation-group wrapper tags from the text', () => {
    const e = decodeEntry(omView()) as { kind: string; text: string };
    expect(e.kind).toBe('om');
    expect(e.text).toContain('user set up a workbench');
    expect(e.text).not.toContain('<observation-group');
    expect(e.text).not.toContain('</observation-group>');
  });

  it('carries the run display fields onto the om entry', () => {
    const e = decodeEntry(omView()) as {
      thinking?: string;
      input?: string;
      suggestedResponse?: string;
      model?: string;
    };
    expect(e.thinking).toBe('deciding what to keep');
    expect(e.input).toBe('the transcript');
    expect(e.suggestedResponse).toBe('walk through it');
    expect(e.model).toBe('dev/qwen3.8-27b');
  });

  it('leaves the display fields undefined when the payload has none', () => {
    const v = omView();
    v.payload = { active_observations: 'plain observation, no group tags' };
    const e = decodeEntry(v) as {
      text: string;
      thinking?: string;
    };
    expect(e.text).toBe('plain observation, no group tags');
    expect(e.thinking).toBeUndefined();
  });
});

describe('resolveBlobs (om)', () => {
  it('substitutes the fetched payload before decode', async () => {
    // The dogfood shape: a 218KB observation stored as sidecar blob 00000063.
    const v: ViewEntry = {
      id: '00000063',
      parent: null,
      kind: 'om',
      timestamp: 1,
      payload: null,
      blob: { id: '00000063', size: 218_000, hash: '0'.repeat(16) },
      first_kept: null
    };
    const views = await resolveBlobs([v], () => Promise.resolve({
      active_observations: 'the observation text'
    }));
    const e = decodeEntry(views[0]) as { kind: string; text: string };
    expect(e.kind).toBe('om');
    expect(e.text).toBe('the observation text');
  });

  it('keeps the pointer when the fetch fails', async () => {
    const v: ViewEntry = {
      id: '00000063',
      parent: null,
      kind: 'om',
      timestamp: 1,
      payload: null,
      blob: { id: '00000063', size: 218_000, hash: '0'.repeat(16) },
      first_kept: null
    };
    const views = await resolveBlobs([v], () => Promise.reject(new Error('no such file')));
    expect(views[0].payload).toBeNull();
    expect(views[0].blob).toBe(v.blob);
  });
});

describe('resolveBlobs (non-blob views)', () => {
  function view(kind: string, payload: unknown, blob: ViewEntry['blob']): ViewEntry {
    return { id: '1', parent: null, kind, timestamp: 1, payload, blob, first_kept: null };
  }

  it('leaves a populated payload alone', async () => {
    const v = view('user', { text: 'hi' }, null);
    const out = await resolveBlobs([v], () => Promise.reject(new Error('must not fetch')));
    expect(out[0].payload).toEqual({ text: 'hi' });
  });

  it('leaves a null payload without a blob pointer alone', async () => {
    const v = view('system', null, null);
    const out = await resolveBlobs([v], () => Promise.reject(new Error('must not fetch')));
    expect(out[0].payload).toBeNull();
  });
});

describe('decodeEntry (user)', () => {
  function userView(payload: Record<string, unknown>): ViewEntry {
    return { id: '1', parent: null, kind: 'user', timestamp: 1, payload, blob: null, first_kept: null };
  }

  it('decodes a plain user message', () => {
    const e = decodeEntry(userView({ text: 'hello' }));
    expect(e).toEqual({ id: '1', kind: 'user', text: 'hello', source: undefined, skill: undefined, msg: undefined });
  });

  it('carries the source and splits a trailing JSON payload into kv', () => {
    const e = decodeEntry(userView({ text: 'done — {"word":"hi"}', source: 's1-1' }));
    if (e.kind !== 'user') throw new Error('not a user entry');
    expect(e.source).toBe('s1-1');
    expect(e.msg).toEqual({ prose: 'done', kv: [{ k: 'word', lines: ['hi'] }] });
  });

  it('leaves msg undefined for a sourced message without a JSON tail', () => {
    const e = decodeEntry(userView({ text: 'plain', source: 's1-1' }));
    if (e.kind !== 'user') throw new Error('not a user entry');
    expect(e.msg).toBeUndefined();
  });

  it('carries a skill ref', () => {
    const e = decodeEntry(userView({ text: 'x', skill: { name: 'deploy', location: '/s/deploy' } }));
    if (e.kind !== 'user') throw new Error('not a user entry');
    expect(e.skill).toEqual({ name: 'deploy', location: '/s/deploy' });
  });
});

describe('decodeEntry (assistant)', () => {
  function assistantView(payload: Record<string, unknown>): ViewEntry {
    return { id: '2', parent: null, kind: 'assistant', timestamp: 1, payload, blob: null, first_kept: null };
  }

  it('decodes a plain message with reasoning, calls and usage', () => {
    const usage = { input_tokens: 1, output_tokens: 2, total_tokens: 3, cached_prompt_tokens: 0 };
    const e = decodeEntry(
      assistantView({ text: 'hi', reasoning: 'r', interrupted: false, calls: [{ call_id: 'c1' }, { call_id: '' }], usage })
    );
    expect(e).toEqual({ id: '2', kind: 'message', text: 'hi', reasoning: 'r', calls: ['c1'], usage });
  });

  it('an interrupted payload decodes to the interrupted kind', () => {
    const e = decodeEntry(assistantView({ text: 'ab', interrupted: true }));
    expect(e.kind).toBe('interrupted');
  });

  it('a missing payload field stays undefined', () => {
    const e = decodeEntry(assistantView({ text: 'hi' }));
    expect(e).toEqual({ id: '2', kind: 'message', text: 'hi', reasoning: undefined, calls: undefined, usage: undefined });
  });
});

describe('decodeEntry (misc kinds)', () => {
  function view(kind: string, payload: Record<string, unknown>): ViewEntry {
    return { id: '9', parent: null, kind, timestamp: 1, payload, blob: null, first_kept: null };
  }

  it('system: the note is the text', () => {
    expect(decodeEntry(view('system', { note: 'model: gpt' }))).toEqual({ id: '9', kind: 'system', text: 'model: gpt' });
  });

  it('spawn-snapshot: the log is the text', () => {
    expect(decodeEntry(view('spawn-snapshot', { log: 'boot log' }))).toEqual({ id: '9', kind: 'spawn-snapshot', text: 'boot log' });
  });

  it('subagent: the payload rides along, the text is its JSON', () => {
    const p = { event: 'state' as const, state: 'done' as const };
    const e = decodeEntry(view('subagent', p));
    expect(e.kind).toBe('subagent');
    if (e.kind === 'subagent') {
      expect(e.payload).toEqual(p);
      expect(e.text).toBe(JSON.stringify(p));
    }
  });

  it('task: the payload rides along, the text is its JSON', () => {
    const p = { event: 'started' as const, id: 't1' };
    const e = decodeEntry(view('task', p));
    expect(e.kind).toBe('task');
    if (e.kind === 'task') {
      expect(e.payload).toEqual(p);
      expect(e.text).toBe(JSON.stringify(p));
    }
  });

  it('an unknown kind renders generically from its payload JSON', () => {
    const e = decodeEntry(view('mystery', { a: 1 }));
    expect(e).toEqual({ id: '9', kind: 'mystery', text: '{"a":1}' });
  });

  it('an empty payload falls back to the id as text', () => {
    const e = decodeEntry(view('mystery', {}));
    const any = e as { text?: string };
    expect(any.text).toBe('{}');
  });
});

describe('decodeEntry (tool, remaining branches)', () => {
  function toolView(payload: Record<string, unknown>): ViewEntry {
    return { id: '3', parent: null, kind: 'tool', timestamp: 1, payload, blob: null, first_kept: null };
  }

  it('a non-object args value decodes to undefined args', () => {
    const e = decodeEntry(toolView({ name: 'bash', args: 'not-an-object', output: 'ok' }));
    if (e.kind !== 'tool') throw new Error('not a tool entry');
    expect(e.args).toBeUndefined();
    expect(e.status).toBe('ok');
  });

  it('a missing name falls back to "tool"', () => {
    const e = decodeEntry(toolView({ output: 'ok' }));
    if (e.kind !== 'tool') throw new Error('not a tool entry');
    expect(e.name).toBe('tool');
  });

  it('an empty-string output is running, not ok', () => {
    const e = decodeEntry(toolView({ name: 'bash', output: '' }));
    if (e.kind !== 'tool') throw new Error('not a tool entry');
    expect(e.status).toBe('running');
  });
});

describe('splitJsonPayload', () => {
  it('splits a trailing JSON object from the prose', () => {
    expect(splitJsonPayload('done — {"a": 1}')).toEqual({
      prose: 'done',
      kv: [{ k: 'a', lines: ['1'] }]
    });
  });

  it('trims trailing whitespace before looking for the object', () => {
    expect(splitJsonPayload('done {"a": 1}  ')).toEqual({
      prose: 'done',
      kv: [{ k: 'a', lines: ['1'] }]
    });
  });

  it('is null when the text does not end with }', () => {
    expect(splitJsonPayload('no payload here')).toBeNull();
  });

  it('is null for a trailing array (not an object)', () => {
    expect(splitJsonPayload('done [1, 2]')).toBeNull();
  });

  it('is null for an empty object (no kv rows)', () => {
    expect(splitJsonPayload('done {}')).toBeNull();
  });

  it('skips unparseable trailing fragments and finds the real object', () => {
    expect(splitJsonPayload('x {bad} {"a": 1}')).toEqual({
      prose: 'x {bad}',
      kv: [{ k: 'a', lines: ['1'] }]
    });
  });

  it('is null when no trailing fragment parses to an object', () => {
    expect(splitJsonPayload('{"a": 1} but not at the end')).toBeNull();
  });
});
