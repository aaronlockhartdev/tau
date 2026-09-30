// @vitest-environment jsdom
// The workspace tag strip: one tag per open workspace, the per-tag ×, the
// focus-mode toggle, and the right-click archive menu that acts on the
// strip's own multiselect.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta, Workspace } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import WorkspaceTabs from './WorkspaceTabs.svelte';
import {
  closeWorkspace,
  closeWorkspaces,
  mockStore,
  openWorkspace,
  resetMockStore,
  seedState,
  tabSelect
} from '../lib/testing/mock-store.svelte';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte');
  return { ...m, store: m.mockStore };
});

const W1: Workspace = { id: 'w1', name: 'alpha', cwd: '/a' };
const W2: Workspace = { id: 'w2', name: 'beta', cwd: '/b' };

const user = userEvent.setup();

function meta(id: string, ws: string): SessionMeta {
  return {
    id,
    workspace: ws,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false
  };
}

type SeedOver = Partial<{
  workspaces: Workspace[];
  sessions: Record<string, SessionState>;
  current: string | null;
  tabSelected: string[];
}>;

function seed(over: SeedOver = {}): void {
  resetMockStore({
    workspaces: [W1, W2],
    sessions: { s1: seedState(meta('s1', 'w1')) },
    current: 's1',
    ...over
  });
}

beforeEach(() => {
  seed();
});

describe('WorkspaceTabs', () => {
  it('renders one tag per open workspace', () => {
    render(WorkspaceTabs);
    expect(screen.getByText('alpha')).toBeInTheDocument();
    expect(screen.getByText('beta')).toBeInTheDocument();
  });

  it('marks the current session’s workspace active', () => {
    render(WorkspaceTabs);
    const tabs = screen.getAllByText(/alpha|beta/).map((el) => el.closest('.tab')!);
    expect(tabs[0]).toHaveClass('active');
    expect(tabs[1]).not.toHaveClass('active');
  });

  it('a plain click selects the tab and opens the workspace', async () => {
    render(WorkspaceTabs);
    await userEvent.click(screen.getByText('alpha'));
    expect(tabSelect).toHaveBeenCalledWith('w1', expect.anything());
    expect(openWorkspace).toHaveBeenCalledWith(W1);
  });

  it('a cmd click joins the selection without opening', async () => {
    render(WorkspaceTabs);
    await user.keyboard('{Meta>}');
    await user.click(screen.getByText('beta'));
    await user.keyboard('{/Meta}');
    expect(tabSelect).toHaveBeenCalledWith('w2', expect.anything());
    expect(openWorkspace).not.toHaveBeenCalled();
  });

  it('the × closes that workspace', async () => {
    render(WorkspaceTabs);
    const xs = screen.getAllByLabelText('Close workspace');
    await userEvent.click(xs[0]);
    expect(closeWorkspace).toHaveBeenCalledWith(W1);
  });

  it('the focus toggle flips store.focus', async () => {
    render(WorkspaceTabs);
    const focus = screen.getByTitle('Focus mode: hide side panes');
    expect(focus).not.toHaveClass('on');
    await userEvent.click(focus);
    expect(mockStore.focus).toBe(true);
    expect(focus).toHaveClass('on');
  });

  it('right-click opens an archive menu for the single tab', async () => {
    render(WorkspaceTabs);
    fireEvent.contextMenu(screen.getByText('alpha'));
    expect(screen.getByRole('menu')).toHaveTextContent('archive');
    await userEvent.click(screen.getByRole('menuitem'));
    expect(closeWorkspaces).toHaveBeenCalledWith([W1]);
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('right-clicking a selected tab archives the whole selection', async () => {
    seed({ tabSelected: ['w1', 'w2'] });
    render(WorkspaceTabs);
    fireEvent.contextMenu(screen.getByText('alpha'));
    expect(screen.getByRole('menuitem')).toHaveTextContent('archive (2)');
    await userEvent.click(screen.getByRole('menuitem'));
    expect(closeWorkspaces).toHaveBeenCalledWith([W1, W2]);
  });

  it('Escape clears the selection and closes the menu', () => {
    seed({ tabSelected: ['w1'] });
    render(WorkspaceTabs);
    fireEvent.contextMenu(screen.getByText('alpha'));
    expect(screen.getByRole('menu')).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(mockStore.tabSelected).toEqual([]);
  });
});
