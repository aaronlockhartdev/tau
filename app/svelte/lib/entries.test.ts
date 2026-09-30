import { describe, expect, it } from 'vitest';
import { decodeEntry, resolveBlobs, upsertEntry } from './entries';
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
