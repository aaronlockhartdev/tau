// @vitest-environment jsdom
// The left pane: the files | sessions tabs. The sessions tab is the MRU
// session tree (children grouped under their parent, archive folder at
// the bottom); the files tab is the workspace listing with a retry note
// when the root fetch failed.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { FileEntry, SessionMeta } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import LeftPane from './LeftPane.svelte';
import {
  mockStore,
  newSession,
  resetMockStore,
  retryDirFetch,
  seedState
} from '../lib/testing/mock-store.svelte';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte');
  return { ...m, store: m.mockStore };
});

const WS = 'w1';

function meta(id: string, over: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id,
    workspace: WS,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false,
    ...over
  };
}

const user = userEvent.setup();

const file = (name: string, path: string, dir = false): FileEntry => ({
  name,
  path,
  dir,
  size: 1
});

type SeedOver = Partial<{
  sessions: Record<string, SessionState>;
  current: string | null;
  files: Record<string, Record<string, FileEntry[]>>;
  fileErrors: Record<string, Record<string, string>>;
}>;

function seed(over: SeedOver = {}): void {
  resetMockStore({
    current: 's1',
    sessions: { s1: seedState(meta('s1')) },
    ...over
  });
}

async function mount(): Promise<void> {
  render(LeftPane);
  // The ensurePane effect creates the pane after the first render.
  await tick();
}

beforeEach(() => {
  cleanup();
  seed();
});

describe('LeftPane', () => {
  it('lists the root files under the files tab', async () => {
    seed({ files: { [WS]: { '.': [file('a.txt', '/a.txt')] } } });
    await mount();
    await user.click(screen.getByRole('button', { name: 'files' }));
    expect(screen.getByText('a.txt')).toBeInTheDocument();
  });

  it('offers a retry note when the root listing failed (click and Enter)', async () => {
    seed({ fileErrors: { [WS]: { '.': 'boom' } } });
    await mount();
    await userEvent.click(screen.getByRole('button', { name: 'files' }));
    const note = screen.getByRole('button', { name: 'failed to list — click to retry' });
    await userEvent.click(note);
    expect(retryDirFetch).toHaveBeenCalledWith(WS, '.');
    retryDirFetch.mockClear();
    fireEvent.keyDown(note, { key: 'Enter' });
    expect(retryDirFetch).toHaveBeenCalledWith(WS, '.');
  });

  it('shows the empty note when nothing is listed', async () => {
    await mount();
    await userEvent.click(screen.getByRole('button', { name: 'files' }));
    expect(screen.getByText('No files listed yet.')).toBeInTheDocument();
  });

  it('the new session button sends newSession for the workspace', async () => {
    await mount();
    await userEvent.click(screen.getByRole('button', { name: /new session/ }));
    expect(newSession).toHaveBeenCalledWith(WS);
  });

  it('lists top-level sessions and groups a child under its active parent', async () => {
    seed({
      sessions: {
        s1: seedState(meta('s1'), { mru: 2 }),
        c1: seedState(meta('c1', { parent: 's1' }), { parent: 's1', mru: 1 })
      }
    });
    await mount();
    expect(screen.getByText('s1')).toBeInTheDocument();
    // The default view expands the active session's chain, so the child is
    // visible without an explicit toggle.
    expect(screen.getByText('c1')).toBeInTheDocument();
  });

  it('lists archived roots behind the archive folder', async () => {
    seed({
      sessions: {
        s1: seedState(meta('s1')),
        s2: seedState(meta('s2', { archived: true }))
      }
    });
    await mount();
    expect(screen.queryByText('s2')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: /archive · 1/ }));
    expect(screen.getByText('s2')).toBeInTheDocument();
  });

  it('switches tabs', async () => {
    await mount();
    await userEvent.click(screen.getByRole('button', { name: 'files' }));
    expect(mockStore.pane[WS].ltab).toBe('files');
    await userEvent.click(screen.getByRole('button', { name: 'sessions' }));
    expect(mockStore.pane[WS].ltab).toBe('sessions');
  });

  it('a click away from the rows clears the selection', async () => {
    await mount();
    const q = mockStore.pane[WS];
    q.selected = ['s1'];
    fireEvent.click(document.body);
    expect(q.selected).toEqual([]);
  });

  it('a shift click spans the selection across the visible rows', async () => {
    seed({
      sessions: {
        s1: seedState(meta('s1'), { mru: 2 }),
        s2: seedState(meta('s2'), { mru: 1 })
      }
    });
    await mount();
    const rows = screen.getAllByText(/s1|s2/).map((el) => el.closest('.trow') as HTMLElement);
    await user.click(rows[0]);
    await user.keyboard('{Shift>}');
    await user.click(rows[1]);
    await user.keyboard('{/Shift}');
    expect(mockStore.pane[WS].selected).toEqual(['s1', 's2']);
});
});
