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

  it('shows a pill per pane tab on each edge', () => {
    render(FocusRails);
    expect(screen.getByTitle('sessions')).toBeTruthy();
    expect(screen.getByTitle('files')).toBeTruthy();
    expect(screen.getByTitle('tasks')).toBeTruthy();
    expect(screen.getByTitle('sub-agents')).toBeTruthy();
  });

  it('a pill opens its side\'s inspector and points the pane at the tab', async () => {
    const user = userEvent.setup();
    render(FocusRails);
    await user.click(screen.getByTitle('files'));
    expect(store.focusInspector).toBe('left');
    expect(store.pane['w1']!.ltab).toBe('files');
    // The inspector carries the real pane.
    expect(screen.getByRole('tablist')).toBeTruthy();
  });

  it('the right rail opens the right inspector on the sub-agents tab', async () => {
    const user = userEvent.setup();
    render(FocusRails);
    await user.click(screen.getByTitle('sub-agents'));
    expect(store.focusInspector).toBe('right');
    expect(store.pane['w1']!.rtab).toBe('subagents');
  });

  it('clicking the active side again closes the inspector', async () => {
    const user = userEvent.setup();
    render(FocusRails);
    const pill = () => screen.getByTitle('files');
    await user.click(pill());
    expect(store.focusInspector).toBe('left');
    await user.click(pill());
    expect(store.focusInspector).toBeNull();
  });
});
