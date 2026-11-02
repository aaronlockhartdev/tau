import { describe, expect, it } from 'vitest';
import { applyDelta, decodeEntry, payloadFor, resolveBlobs, toolStatus, upsertEntry } from './entries';
import type { BlobRef, Entry, EntryPayload, ViewEntry } from './protocol';

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
    payload: payloadFor('om', payload),
    blob: null,
    first_kept: null
  };
}

describe('upsertEntry (ADR-0008)', () => {
  it('a first sight creates the card under its id', () => {
    const entries: Record<string, Entry> = {};
    upsertEntry(entries, omView());
    expect(Object.keys(entries)).toEqual(['5']);
    expect(entries['5']!.kind).toBe('om');
  });

  it('a re-emission updates the slot in place and never repositions it', () => {
    const v = (id: string, text: string): ViewEntry => ({
      id,
      parent: null,
      kind: 'assistant',
      timestamp: 1,
      payload: payloadFor('assistant', { text, reasoning: 'r' }),
      blob: null,
      first_kept: null
    });
    const entries: Record<string, Entry> = {};
    upsertEntry(entries, v('00000001', 'hel'));
    upsertEntry(entries, v('00000002', 'other'));
    upsertEntry(entries, v('00000001', 'hello'));
    expect(Object.keys(entries)).toEqual(['00000001', '00000002']);
    const e = entries['00000001']!;
    if (e.kind === 'message' || e.kind === 'interrupted') expect(e.text).toBe('hello');
  });
});

describe('decodeEntry (how the response ended, #307)', () => {
  const assistant = (payload: Record<string, unknown>): ViewEntry => ({
    id: '00000001',
    parent: null,
    kind: 'assistant',
    timestamp: 1,
    payload: payloadFor('assistant', payload),
    blob: null,
    first_kept: null
  });

  it('reads the ending the provider named', () => {
    const e = decodeEntry(
      assistant({ text: 'half', interrupted: false, stop_reason: 'length' })
    ) as { kind: string; endedBy?: string };
    expect(e.kind).toBe('message');
    expect(e.endedBy).toBe('length');
  });

  it('an ending this build does not know is no ending', () => {
    const e = decodeEntry(assistant({ text: 'x', stop_reason: 'exploded' })) as {
      endedBy?: string;
    };
    expect(e.endedBy).toBeUndefined();
  });
});

// #303: the wire carries only the new characters, so the view owns the
// accumulation and the seq numbers decide what applies.
const view = (entries: Record<string, Entry>, seqs: Map<string, number>, seq: number, text: string, reasoning = '', id = '00000001') =>
  applyDelta(entries, seqs, { id, seq, text, reasoning });

describe('applyDelta (#303)', () => {
  it('a run of deltas tiles the stream into one card', () => {
    const entries: Record<string, Entry> = {};
    const seqs = new Map<string, number>();
    expect(view(entries, seqs, 1, 'hel', 'thin')).toBe(true);
    expect(view(entries, seqs, 2, 'lo')).toBe(true);
    expect(view(entries, seqs, 3, ' — done', 'k')).toBe(true);
    const e = entries['00000001']!;
    expect(e.kind).toBe('message');
    if (e.kind === 'message') {
      expect(e.text).toBe('hello — done');
      expect(e.reasoning).toBe('think');
    }
    expect(Object.keys(entries)).toEqual(['00000001']);
  });

  it('the first delta creates the card the file line later replaces', () => {
    const entries: Record<string, Entry> = {};
    const seqs = new Map<string, number>();
    view(entries, seqs, 1, 'first');
    expect(entries['00000001']?.kind).toBe('message');
    upsertEntry(entries, {
      id: '00000001',
      parent: null,
      kind: 'assistant',
      timestamp: 9,
      payload: payloadFor('assistant', { text: 'first line', interrupted: false, usage: { input_tokens: 1, output_tokens: 2, total_tokens: 3 } }),
      blob: null,
      first_kept: null
    });
    const e = entries['00000001']!;
    if (e.kind === 'message') {
      expect(e.text).toBe('first line');
      expect(e.usage?.total_tokens).toBe(3);
    }
  });

  it('a gap is refused: the missing frame is not papered over', () => {
    const entries: Record<string, Entry> = {};
    const seqs = new Map<string, number>();
    view(entries, seqs, 1, 'one ');
    expect(view(entries, seqs, 3, 'three ')).toBe(false);
    expect((entries['00000001'] as Entry & { text: string }).text).toBe('one ');
    // The run stays frozen: the view never guesses a seq, and the entry's
    // file line is the recovery for everything the gap lost.
    expect(view(entries, seqs, 4, 'four ')).toBe(false);
  });

  it('a duplicate seq is refused: the text never repeats', () => {
    const entries: Record<string, Entry> = {};
    const seqs = new Map<string, number>();
    view(entries, seqs, 1, 'once');
    expect(view(entries, seqs, 1, 'once')).toBe(false);
    expect((entries['00000001'] as Entry & { text: string }).text).toBe('once');
  });

  it('a stream that opens mid-run creates no card', () => {
    const entries: Record<string, Entry> = {};
    const seqs = new Map<string, number>();
    expect(view(entries, seqs, 2, 'fragment')).toBe(false);
    expect(Object.keys(entries)).toEqual([]);
  });

  it('a delta for an entry the view already committed is refused', () => {
    const entries: Record<string, Entry> = {};
    // The committed file line: the store cleared the tracker when it landed.
    upsertEntry(entries, {
      id: '00000001',
      parent: null,
      kind: 'assistant',
      timestamp: 9,
      payload: payloadFor('assistant', { text: 'the record', interrupted: true }),
      blob: null,
      first_kept: null
    });
    const seqs = new Map<string, number>();
    expect(view(entries, seqs, 4, 'late')).toBe(false);
    expect(entries['00000001']?.kind).toBe('interrupted');
    expect((entries['00000001'] as Entry & { text: string }).text).toBe('the record');
  });

  it('a reasoning-only delta leaves the text empty, not absent', () => {
    const entries: Record<string, Entry> = {};
    const seqs = new Map<string, number>();
    expect(view(entries, seqs, 1, '', 'thinking')).toBe(true);
    const e = entries['00000001'] as Entry & { text: string; reasoning?: string };
    expect(e.text).toBe('');
    expect(e.reasoning).toBe('thinking');
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
    v.payload = payloadFor('om', { active_observations: 'plain observation, no group tags' });
    const e = decodeEntry(v) as {
      text: string;
      thinking?: string;
    };
    expect(e.text).toBe('plain observation, no group tags');
    expect(e.thinking).toBeUndefined();
  });
});

// #346: the payload crosses the seam typed — the adjacent `{type, data}` tag,
// with `other` as the escape arm for a payload the core could not type.
describe('decodeEntry (the typed payload, #346)', () => {
  const view = (kind: string, payload: EntryPayload | null, blob: BlobRef | null = null): ViewEntry => ({
    id: '00000001',
    parent: null,
    kind,
    timestamp: 1,
    payload,
    blob,
    first_kept: null
  });

  it('reads a field out of the arm the core typed', () => {
    const e = decodeEntry(
      view('assistant', { type: 'assistant', data: { text: 'typed', reasoning: 'r', interrupted: false, calls: [] } })
    ) as { kind: string; text: string };
    expect(e.kind).toBe('message');
    expect(e.text).toBe('typed');
  });

  it('reads a tool arm with its GUI-only channel', () => {
    const e = decodeEntry(
      view('tool', {
        type: 'tool',
        data: { call_id: 'c1', name: 'edit', args: { path: 'a.txt' }, output: 'edited', ui_detail: '--- a\n+++ b', status: 'ok' }
      })
    ) as { name?: string; output?: string; ui_detail?: string; status?: string };
    expect(e.name).toBe('edit');
    expect(e.ui_detail).toBe('--- a\n+++ b');
    expect(e.status).toBe('ok');
  });

  it('the payload tag is serde\'s, not the kind\'s hyphenated spelling', () => {
    const e = decodeEntry(
      view('spawn-snapshot', { type: 'spawn_snapshot', data: { parentSession: 'p', range: '0:1', log: 'the frozen prefix' } })
    ) as { kind: string; text: string };
    expect(e.kind).toBe('spawn-snapshot');
    expect(e.text).toBe('the frozen prefix');
  });

  // The negative: the core keeps a payload it cannot type in `other`, with its
  // bytes intact. The card must read those bytes, not blank on the tag miss.
  it('the escape arm renders the raw map', () => {
    const e = decodeEntry(view('user', { type: 'other', data: { text: 'a core newer than this build' } })) as {
      kind: string;
      text: string;
    };
    expect(e.kind).toBe('user');
    expect(e.text).toBe('a core newer than this build');
  });

  // The other negative: a payload whose arm is not the kind the entry claims —
  // the card reads defaults from an empty map, it never renders a field of the
  // wrong shape as if it belonged.
  it('an arm under the wrong kind reads as empty, not as the other kind\'s fields', () => {
    const e = decodeEntry(view('user', { type: 'system', data: { note: 'a note' } })) as { text: string };
    expect(e.text).toBe('');
  });

  it('a sidecar payload is tagged by kind on the way in', async () => {
    const v = view('tool', null, { id: '00000001', size: 218_000, hash: '0'.repeat(16) });
    const views = await resolveBlobs([v], () => Promise.resolve({ name: 'read', call_id: 'c1', args: {}, output: 'file' }));
    const e = decodeEntry(views[0]!) as { name?: string; output?: string; unavailable?: boolean };
    expect(e.name).toBe('read');
    expect(e.output).toBe('file');
    expect(e.unavailable).toBeUndefined();
  });

  it('a kind the payload module has no arm for tags as other', () => {
    expect(payloadFor('quantum', { note: 'x' })).toEqual({ type: 'other', data: { note: 'x' } });
  });
});

describe('decodeEntry (tool status, B5)', () => {
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
    const e = decodeEntry(views[0]!) as { kind: string; text: string };
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
    expect(views[0]!.payload).toBeNull();
    expect(views[0]!.blob).toBe(v.blob);
  });
});

describe('decodeEntry (tool status, B5)', () => {
  function toolView(status: unknown, output: unknown, ui_detail: unknown = undefined): ViewEntry {
    return {
      id: 't1',
      parent: null,
      kind: 'tool',
      timestamp: 1,
      payload: payloadFor('tool', { name: 'read', output, status, ui_detail }),
      blob: null,
      first_kept: null
    };
  }
  const decoded = (v: ViewEntry) => (decodeEntry(v) as { status?: string }).status;

  it('prefers the core-emitted status over the prefix heuristic', () => {
    // "read: ..." is an error by the heuristic, but the explicit ok wins.
    expect(decoded(toolView('ok', 'read: cannot read /x'))).toBe('ok');
    expect(decoded(toolView('error', 'file contents'))).toBe('error');
  });

  it('decodes the GUI-only ui_detail channel (#243)', () => {
    const withDetail = decodeEntry(
      toolView('ok', 'wrote a.txt (7 bytes, 2 lines)', 'alpha\nbeta')
    ) as { ui_detail?: string; output?: string };
    expect(withDetail.ui_detail).toBe('alpha\nbeta');
    expect(withDetail.output).toBe('wrote a.txt (7 bytes, 2 lines)');
    // A wire-absent field (a non-split tool, or a pre-#243 line) stays undefined.
    const bare = decodeEntry(toolView('ok', 'exit 0')) as { ui_detail?: string };
    expect(bare.ui_detail).toBeUndefined();
  });

  it('honors an explicit cancelled status (a user stop, #222)', () => {
    // The heuristic would read the "bash: " prefix as an error; the core's
    // explicit cancelled wins so the card renders neutral, not red.
    const bashView = (status: unknown) =>
      decodeEntry({ ...toolView(status, 'bash: cancelled by user stop'), payload: payloadFor('tool', { name: 'bash', output: 'bash: cancelled by user stop', status }) });
    expect((bashView('cancelled') as { status?: string }).status).toBe('cancelled');
  });

  it('falls back to the heuristic when no explicit status is present', () => {
    expect(decoded(toolView(undefined, 'read: cannot read /x'))).toBe('error');
    expect(decoded(toolView(undefined, 'file contents'))).toBe('ok');
    expect(decoded(toolView(undefined, undefined))).toBe('running');
    // A bash cancellation record without the explicit status (pre-#222 shape)
    // still reads as an error by the "bash: " prefix heuristic.
    expect(
      decoded({
        ...toolView(undefined, 'bash: cancelled by user stop'),
        payload: payloadFor('tool', { name: 'bash', output: 'bash: cancelled by user stop' })
      })
    ).toBe('error');
  });
});

describe('toolStatus', () => {
  it('a non-bash tool is a failure when the output starts with its own name', () => {
    expect(toolStatus('read', 'read: cannot read /x: no such file')).toBe('error');
  });
  it('a non-bash tool result content is success', () => {
    expect(toolStatus('read', 'file contents here')).toBe('ok');
  });
  it('a dispatch-level failure is an error', () => {
    expect(toolStatus('read', 'unknown tool: read')).toBe('error');
  });
  it('bash exit 0 is ok and non-zero is error', () => {
    expect(toolStatus('bash', 'exit 0\n--- stdout ---\nhello')).toBe('ok');
    expect(toolStatus('bash', 'exit 3\n--- stderr ---\noops')).toBe('error');
  });
  it('a bash spawn/timeout/stop failure is an error', () => {
    expect(toolStatus('bash', 'bash: timed out after 60s')).toBe('error');
    expect(toolStatus('bash', 'bash: interrupted by user stop')).toBe('error');
  });
  it('in-flight bash streaming output (no exit/bash: prefix) is running, not ok (#197)', () => {
    expect(toolStatus('bash', 'hello\n')).toBe('running');
    expect(toolStatus('bash', 'hello\nworld\n')).toBe('running');
  });
});

describe('a diagnostic entry (ADR-0034)', () => {
  function diagnosticView(payload: unknown): ViewEntry {
    return {
      id: '9',
      parent: null,
      kind: 'diagnostic',
      timestamp: 123,
      payload: payloadFor('diagnostic', payload),
      blob: null,
      first_kept: null
    };
  }

  it('decodes the tagged payload into the typed fact', () => {
    const e = decodeEntry(diagnosticView({
      code: 'turn_cutoff',
      model: 'dev/qwen3.8-flash',
      ended_by: 'eof_without_done',
      blocks: 2
    }));
    if (!('diagnostic' in e)) throw new Error('expected a diagnostic entry');
    expect(e.diagnostic).toEqual({
      code: 'turn_cutoff',
      model: 'dev/qwen3.8-flash',
      ended_by: 'eof_without_done',
      blocks: 2
    });
    // The preview line carries the code, never a stored sentence.
    expect(e.text).toBe('turn_cutoff');
  });

  it('a code this build does not know survives with its name', () => {
    const e = decodeEntry(diagnosticView({ code: 'tool_stalled', reason: 'watchdog' }));
    if (!('diagnostic' in e)) throw new Error('expected a diagnostic entry');
    expect(e.diagnostic.code).toBe('tool_stalled');
    expect(e.text).toBe('tool_stalled');
  });

  // The negative: a payload that is not a diagnostic map at all must still
  // render something — never the blank card #306 is about.
  it('a payload with no code decodes rather than blanking', () => {
    const e = decodeEntry(diagnosticView({ prompt: 1200 }));
    if (!('diagnostic' in e)) throw new Error('expected a diagnostic entry');
    expect(e.diagnostic).toEqual({ code: 'unknown' });
    expect(e.text).toBe('unknown');
  });
});

// A kind this build does not know (ADR-0008 tolerance): the line still
// decodes and says what it is, rather than vanishing from the transcript.
it('an unknown kind renders its own name', () => {
  const e = decodeEntry({
    id: '10',
    parent: null,
    kind: 'quantum',
    timestamp: 123,
    payload: payloadFor('quantum', { note: 'from a newer core' }),
    blob: null,
    first_kept: null
  });
  expect(e.kind).toBe('quantum');
  expect(e.text ?? '').toContain('from a newer core');
});
