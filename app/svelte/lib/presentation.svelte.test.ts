// @vitest-environment jsdom
// The presentation registry (spec §12, #35): per-user prefs, localStorage-
// backed, with the §6 defaults.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { presentation, setPresentation } from './presentation.svelte';

const KEY = 'tau.presentation';
const DEFAULTS = {
  reasoningVisibleByDefault: true,
  hideThinking: false,
  showCacheMiss: true
};

describe('presentation prefs', () => {
  beforeEach(() => {
    localStorage.removeItem(KEY);
    setPresentation(DEFAULTS);
  });

  it('starts on the §6 defaults', () => {
    expect(presentation).toEqual(DEFAULTS);
  });

  it('persists to localStorage on change', () => {
    setPresentation({ hideThinking: true });
    const raw = JSON.parse(localStorage.getItem(KEY) ?? '');
    expect(raw).toEqual({
      reasoningVisibleByDefault: true,
      hideThinking: true,
      showCacheMiss: true
    });
  });

  it('merges a stored file over the defaults (older builds lack new keys)', async () => {
    localStorage.setItem(KEY, JSON.stringify({ hideThinking: true }));
    vi.resetModules();
    const fresh = await import('./presentation.svelte');
    expect(fresh.presentation).toEqual({
      reasoningVisibleByDefault: true,
      hideThinking: true,
      showCacheMiss: true
    });
  });

  it('falls back to the defaults on corrupt storage', async () => {
    localStorage.setItem(KEY, 'not json');
    vi.resetModules();
    const fresh = await import('./presentation.svelte');
    expect(fresh.presentation).toEqual(DEFAULTS);
  });
});
