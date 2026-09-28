import { describe, expect, it } from 'vitest';
import { collapseToolCalls, decodeEntry, resolveBlobs } from './entries';
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

// A two-phase tool record as the file carries it: the call entry (empty
// output, recorded before dispatch) and the result entry (same call_id).
function toolView(id: string, callId: string, output: string): ViewEntry {
  return {
    id,
    parent: null,
    kind: 'tool',
    timestamp: 1,
    payload: { call_id: callId, name: 'task_create', args: { title: 't' }, output },
    blob: null,
    first_kept: null
  };
}

describe('collapseToolCalls (two-phase tool record)', () => {
  it('folds a call + result pair into the call slot, keeping its position', () => {
    const entries: Entry[] = [
      { id: '1', kind: 'user', text: 'go' },
      decodeEntry(toolView('2', 'c1', '')),
      { id: '3', kind: 'task', text: 'task: t', payload: { event: 'created' } } as Entry,
      decodeEntry(toolView('4', 'c1', 'created 1'))
    ];
    const out = collapseToolCalls(entries);
    expect(out).toHaveLength(3);
    expect(out[1]).toMatchObject({ id: '2', kind: 'tool', output: 'created 1' });
    expect(out[2]).toMatchObject({ id: '3', kind: 'task' });
  });

  it('leaves a lone result (pre-change file) untouched', () => {
    const entries: Entry[] = [decodeEntry(toolView('2', 'c1', 'created 1'))];
    expect(collapseToolCalls(entries)).toHaveLength(1);
  });

  it('keeps each pair to its own call_id', () => {
    const entries: Entry[] = [
      decodeEntry(toolView('2', 'c1', '')),
      decodeEntry(toolView('3', 'c1', 'out1')),
      decodeEntry(toolView('4', 'c2', '')),
      decodeEntry(toolView('5', 'c2', 'out2'))
    ];
    const out = collapseToolCalls(entries);
    expect(out).toHaveLength(2);
    expect(out[0]).toMatchObject({ id: '2', output: 'out1' });
    expect(out[1]).toMatchObject({ id: '4', output: 'out2' });
  });

  it('drops a re-read of a result whose call slot already shows the output', () => {
    const entries: Entry[] = [
      { id: '2', kind: 'tool', call_id: 'c1', output: 'out1', status: 'ok' } as Entry,
      { id: '4', kind: 'tool', call_id: 'c2', output: 'out2', status: 'ok' } as Entry,
      decodeEntry(toolView('3', 'c1', 'out1'))
    ];
    expect(collapseToolCalls(entries)).toHaveLength(2);
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
    const views = await resolveBlobs([v], async () => ({
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
