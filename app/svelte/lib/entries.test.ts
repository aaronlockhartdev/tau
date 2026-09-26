import { describe, expect, it } from 'vitest';
import { decodeEntry } from './entries';
import type { ViewEntry } from './protocol';

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
