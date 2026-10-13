// The focus-mode rails: icon buttons open the non-modal inspector sheets.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import FocusRails from './FocusRails.svelte';
import { mockStore as store, resetMockStore, seedState, ensurePane } from '../lib/testing/mock-store.svelte';
import type { SessionMeta } from '../lib/protocol';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const meta = (id: string, over: Partial<SessionMeta> = {}): SessionMeta => ({
  id,
  workspace: over.workspace ?? 'w1',
  title: over.title ?? null,
  parent: null,
  created: 1000,
  leaf: null,
  model: null,
  usage: null,
  archived: false,
  ...over
});

beforeEach(() => {
  cleanup();
  resetMockStore({
    current: 'a',
    sessions: { a: seedState(meta('a', { title: 'alpha' })) }
  });
  ensurePane('w1');
  store.focus = true;
});

describe('FocusRails', () => {
  it('renders nothing outside focus mode', () => {
    store.focus = false;
    render(FocusRails);
    expect(screen.queryByTitle('sessions')).toBeNull();
  });

  it('shows one drawer handle per edge', () => {
    render(FocusRails);
    expect(screen.getByTitle('show files & sessions')).toBeTruthy();
    expect(screen.getByTitle('show tasks & sub-agents')).toBeTruthy();
  });

  it('a handle opens its side\'s inspector with the real pane', async () => {
    const user = userEvent.setup();
    render(FocusRails);
    await user.click(screen.getByTitle('show files & sessions'));
    expect(store.focusInspector).toBe('left');
    // The inspector carries the real pane (with its own tab header).
    expect(screen.getByRole('tablist')).toBeTruthy();
  });

  it('the right handle opens the right inspector', async () => {
    const user = userEvent.setup();
    render(FocusRails);
    await user.click(screen.getByTitle('show tasks & sub-agents'));
    expect(store.focusInspector).toBe('right');
  });

  it('clicking the active side again closes the inspector', async () => {
    const user = userEvent.setup();
    render(FocusRails);
    const handle = () => screen.getByTitle('show files & sessions');
    await user.click(handle());
    expect(store.focusInspector).toBe('left');
    await user.click(handle());
    expect(store.focusInspector).toBeNull();
  });
});
