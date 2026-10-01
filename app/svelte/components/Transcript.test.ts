// @vitest-environment jsdom
// The transcript: the virtualized entry list with stick-to-bottom pin and
// paged hydration. virtua is mocked (jsdom has no layout to window
// against); the paged-read logic is driven through the mock's scroll
// driver registry.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import type { Entry, SessionMeta } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import Transcript from './Transcript.svelte';
import { drives } from '../lib/testing/VirtualizerMock.svelte';
import { fetchWindow, mockStore, resetMockStore, seedState } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});
vi.mock('virtua/svelte', async () => ({
  Virtualizer: (await import('../lib/testing/VirtualizerMock.svelte')).default
}));

const WS = 'w1';

function meta(id: string): SessionMeta {
  return {
    id,
    workspace: WS,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false
  };
}

function entries(n: number): Record<string, Entry> {
  const out: Record<string, Entry> = {};
  for (let i = 0; i < n; i++) out[`e${i}`] = { id: `e${i}`, kind: 'user', text: `message ${i}` };
  return out;
}

type SeedOver = { turn?: 'running' | 'idle' | 'starting' };

function seed(n: number, over: SeedOver = {}): void {
  const s: SessionState = seedState(meta('c1'));
  s.entries = entries(n);
  if (over.turn) s.turn = over.turn;
  resetMockStore({ current: 'c1', sessions: { c1: s } });
}

async function mount(): Promise<void> {
  render(Transcript);
  await tick();
}

beforeEach(() => {
  seed(0);
});

describe('Transcript', () => {
  it('shows the placeholder when there are no entries', async () => {
    await mount();
    expect(screen.getByText('no entries yet')).toBeInTheDocument();
  });

  it('renders the materialized entries', async () => {
    seed(2);
    await mount();
    expect(screen.getByText('message 0')).toBeInTheDocument();
    expect(screen.getByText('message 1')).toBeInTheDocument();
  });

  it('shows the waiting dots while a turn awaits first output', async () => {
    seed(1, { turn: 'starting' });
    const { container } = render(Transcript);
    await tick();
    expect(container.querySelector('.waiting')).not.toBeNull();
  });

  it('opens pinned at the tail: the first page is the last 20', async () => {
    seed(30);
    await mount();
    expect(fetchWindow).toHaveBeenCalledWith('c1', 10, 20);
  });

  it('re-issues the tail when a closed session reopens', async () => {
    seed(30);
    await mount();
    fetchWindow.mockClear();
    // A close drops the session and the current: the one-shot must not
    // survive a reopen of the same id.
    resetMockStore({ current: null, sessions: {} });
    await tick();
    seed(30);
    await tick();
    expect(fetchWindow).toHaveBeenCalledTimes(1);
    expect(fetchWindow).toHaveBeenCalledWith('c1', 10, 20);
  });

  it('fetches a page when the visible range drifts beyond the hysteresis margin', async () => {
    seed(30);
    await mount();
    fetchWindow.mockClear();
    drives[0]!(0);
    await tick();
    expect(fetchWindow).toHaveBeenCalledTimes(1);
    expect(fetchWindow).toHaveBeenCalledWith('c1', 0, 0);
    expect(mockStore.renderRange).toBe('0–0 of 30');
    // A second scroll inside the margin must not re-fetch.
    drives[0]!(0);
    expect(fetchWindow).toHaveBeenCalledTimes(1);
  });

  it('re-reads the in-turn range when the turn ends', async () => {
    seed(30, { turn: 'running' });
    await mount();
    fetchWindow.mockClear();
    mockStore.sessions.c1!.turn = 'idle';
    await tick();
    // the one-shot suppresses the open-tail re-issue; only the union re-read lands
    expect(fetchWindow).toHaveBeenCalledTimes(1);
    expect(fetchWindow).toHaveBeenCalledWith('c1', 0, 30);
  });

  it('unions a mid-turn scroll into the turn-end re-read', async () => {
    seed(30, { turn: 'running' });
    await mount();
    fetchWindow.mockClear();
    // A scroll while the turn runs widens the mid-turn union beyond the
    // open-tail page; the turn-end re-read covers the whole union. The
    // open-tail effect re-fires on the turn change (it reads the turn for
    // the union) but the one-shot suppresses the re-issue.
    drives[0]!(0);
    await tick();
    mockStore.sessions.c1!.turn = 'idle';
    await tick();
    expect(fetchWindow).toHaveBeenCalledTimes(2);
    expect(fetchWindow).toHaveBeenCalledWith('c1', 0, 0);
    expect(fetchWindow).toHaveBeenCalledWith('c1', 0, 30);
  });

  it('prunes its expansion state when the session is dropped', async () => {
    seed(1);
    mockStore.entryOpen.set('c1:e0:tool', true);
    const { unmount } = render(Transcript);
    await tick();
    resetMockStore({ current: 'c2', sessions: { c2: seedState(meta('c2')) } });
    unmount();
    expect(mockStore.entryOpen.has('c1:e0:tool')).toBe(false);
  });
});
