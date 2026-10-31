import { describe, expect, it } from 'vitest';

import { diagnosticLabel } from './diagnostics';
import type { Entry } from '../wire/protocol';

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
});

// The escape arm beside it: a code this build never heard of renders by its own
// name, never as a blank card.
describe('a code this build does not know', () => {
  it('renders by its code', () => {
    const entry: Entry = { id: 'e2', kind: 'diagnostic', text: 'future_code', diagnostic: { code: 'future_code' } };
    expect(diagnosticLabel(entry)).toBe('future_code');
  });
});
