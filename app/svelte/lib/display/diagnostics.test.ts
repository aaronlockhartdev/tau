import { describe, expect, it } from 'vitest';

import { diagnosticLabel } from './diagnostics';
import { MERGED_CONFIG_SENTINEL } from '../wire/protocol';
import type { Diagnostic, DiagnosticCode, Entry } from '../wire/protocol';

// A config the core could not read is inert, so its card names the file and
// repeats what the loader said (#359).
const rejected = (path: string, reason: string): Entry => ({
  id: 'e1',
  kind: 'diagnostic',
  text: 'config_rejected',
  diagnostic: { code: 'config_rejected', path, reason }
});

describe('the config_rejected card', () => {
  it('names the file and the load failure', () => {
    expect(
      diagnosticLabel(
        rejected('/tmp/tau/.tau/config.toml', 'parsing /tmp/tau/.tau/config.toml: missing field `base_url`')
      )
    ).toBe('config not applied (/tmp/tau/.tau/config.toml): parsing /tmp/tau/.tau/config.toml: missing field `base_url`');
  });

  it('shows only the first line of a multi-line reason', () => {
    expect(diagnosticLabel(rejected('/tmp/tau/.tau/config.toml', 'toml parse error\nat line 3'))).toBe(
      'config not applied (/tmp/tau/.tau/config.toml): toml parse error'
    );
  });

  // A load rule is the merged config's property and names no file, so the core
  // sends the sentinel: the card says the merged config, never a path the user
  // did not edit (#366).
  it('names the merged config for a rule failure, not a file', () => {
    const reason = 'thinking.levels.a/b: no provider declares it as a model, so the level is never read';
    expect(diagnosticLabel(rejected(MERGED_CONFIG_SENTINEL, reason)))
      .toBe(`config not applied (the merged config): ${reason}`);
  });
});

// docs/protocol.md says every code the wire carries has a card. This record is
// that list typed: a code added to `DiagnosticCode` with no key here fails the
// type-check, and the test below refuses a code the GUI renders bare — the pin
// the prose list has no runtime of its own (#359).
const EVERY_CODE: Record<DiagnosticCode, Diagnostic> = {
  model_changed: { code: 'model_changed', to: 'aaa/m' },
  gate_unavailable: { code: 'gate_unavailable', model: 'aaa/m', missing: ['context_window'] },
  context_overflow: { code: 'context_overflow', prompt: 9000, budget: 8000 },
  gate_attempt: { code: 'gate_attempt', command: 'cargo test', retries: 1, max: 3, output: 'failed' },
  gate_failed: { code: 'gate_failed', command: 'cargo test', retries: 3 },
  om_stalled: { code: 'om_stalled', pending: 1200, activation: 1000 },
  om_failed: { code: 'om_failed', stage: 'observe', reason: 'provider refused' },
  provider_rejected: { code: 'provider_rejected', status: 400, body: 'bad request' },
  missing_user_query: { code: 'missing_user_query', items: 2 },
  turn_cutoff: { code: 'turn_cutoff', model: 'aaa/m', ended_by: 'length', blocks: 2 },
  config_rejected: { code: 'config_rejected', path: '/tmp/x/.tau/config.toml', reason: 'missing field `base_url`' }
};

describe('the wire\'s code list', () => {
  it('renders a sentence for every code, never the bare code', () => {
    for (const [code, payload] of Object.entries(EVERY_CODE) as [DiagnosticCode, Diagnostic][]) {
      const card: Entry = { id: `e-${code}`, kind: 'diagnostic', text: code, diagnostic: payload };
      expect(diagnosticLabel(card), `${code} renders bare`).not.toBe(code);
    }
  });
});

// The escape arm beside the list: a code this build never heard of renders by
// its own name, never as a blank card.
describe('a code this build does not know', () => {
  it('renders by its code', () => {
    const entry: Entry = { id: 'e2', kind: 'diagnostic', text: 'future_code', diagnostic: { code: 'future_code' } };
    expect(diagnosticLabel(entry)).toBe('future_code');
  });
});
