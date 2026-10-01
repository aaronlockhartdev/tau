// @vitest-environment jsdom
// One session-tree node: the row content (title, badge, mru), the click
// grammar (plain opens, cmd toggles the selection, shift spans), the
// inline rename, and the right-click menu that acts on the selection.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta, SubagentInfo } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import SessionNode from './SessionNode.svelte';
import {
  archiveSession,
  deleteSession,
  ensurePane,
  mockStore,
  openSessionById,
  resetMockStore,
  restoreSession,
  renameSession,
  seedState
} from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const WS = 'w1';
const user = userEvent.setup();
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

// The visible row order the shift range spans.
const visibleIds = ['s1'];

type SeedOver = {
  sessions?: Record<string, SessionState>;
  current?: string | null;
};

function seed(over: SeedOver = {}): SessionState {
  const self = seedState(meta('s1'));
  const sessions = over.sessions ?? { s1: self };
  resetMockStore({ current: over.current ?? 's1', sessions });
  // The component reads pane(ws) directly: materialize it before render.
  ensurePane(WS);
  return self;
}

async function mount(
  node: SessionState,
  props: { depth?: number; sessions?: SessionState[] } = {}
): Promise<void> {
  render(SessionNode, {
    props: {
      session: node,
      depth: props.depth ?? 0,
      ws: WS,
      sessions: props.sessions ?? [node],
      visibleIds
    }
  });
  await tick();
}

const rowOf = (text: string): HTMLElement =>
  screen.getByText(text).closest('.trow') as HTMLElement;

beforeEach(() => {
  seed();
});

describe('SessionNode', () => {
  it('renders the title (falling back to the id) and a fresh mru', async () => {
    const s = seed();
    s.meta.title = 'my session';
    s.mru = Date.now();
    await mount(s);
    expect(screen.getByText('my session')).toBeInTheDocument();
    expect(screen.getByText('just now')).toBeInTheDocument();
  });

  it('a running top-level row gets the running badge', async () => {
    const s = seed();
    s.turn = 'running';
    await mount(s);
    const badge = screen.getByText('running');
    expect(badge.closest('.badge')).toHaveClass('running');
  });

  it('a child row shows its lifecycle state and declared wait', async () => {
    const s = seed();
    s.parent = 'p1';
    s.state = 'idle';
    s.waiting_on = 'parent';
    const info: SubagentInfo = {
      handle: 'h1',
      child: 's1',
      agent_type: 'worker',
      context_mode: 'fresh',
      state: 'idle',
      waiting_on: 'parent',
      last_message: null,
      usage: null,
      task: null,
      resume_contract: null
    };
    s.subagents = [info];
    await mount(s, { depth: 1 });
    expect(screen.getByText('idle · parent')).toBeInTheDocument();
  });

  it('a plain click opens the session and sets the anchor', async () => {
    await mount(seed());
    await userEvent.click(rowOf('s1'));
    expect(openSessionById).toHaveBeenCalledWith('s1');
    const q = mockStore.pane[WS];
    expect(q.selAnchor).toBe('s1');
    expect(q.selected).toEqual([]);
  });

  it('a cmd click toggles the selection without opening', async () => {
    await mount(seed());
    await user.keyboard('{Meta>}');
    await user.click(rowOf('s1'));
    await user.keyboard('{/Meta}');
    expect(openSessionById).not.toHaveBeenCalled();
    expect(mockStore.pane[WS].selected).toEqual(['s1']);
  });

  it('a shift click spans from the anchor', async () => {
    await mount(seed());
    mockStore.pane[WS].selAnchor = 's1';
    await user.keyboard('{Shift>}');
    await user.click(rowOf('s1'));
    await user.keyboard('{/Shift}');
    expect(mockStore.pane[WS].selected).toEqual(['s1']);
    expect(openSessionById).not.toHaveBeenCalled();
  });

  it('an archived row never opens', async () => {
    const s = seed();
    s.archived = true;
    await mount(s);
    await userEvent.click(rowOf('s1'));
    expect(openSessionById).not.toHaveBeenCalled();
  });

  it('the archive button archives the row', async () => {
    await mount(seed());
    await userEvent.click(screen.getByLabelText('archive session'));
    expect(archiveSession).toHaveBeenCalledWith('s1');
  });

  it('double-click starts an inline rename; Enter commits it', async () => {
    await mount(seed());
    await userEvent.dblClick(rowOf('s1'));
    const input = screen.getByLabelText('rename session');
    await userEvent.type(input, 'alpha');
    await userEvent.keyboard('{Enter}');
    expect(renameSession).toHaveBeenCalledWith('s1', 'alpha');
  });

  it('Escape cancels the rename', async () => {
    await mount(seed());
    mockStore.pane[WS].renamingId = 's1';
    await tick();
    const input = screen.getByLabelText('rename session');
    await userEvent.keyboard('{Escape}');
    expect(mockStore.pane[WS].renamingId).toBeNull();
    expect(input).not.toBeInTheDocument();
  });

  it('right-click offers archive for a live row', async () => {
    await mount(seed());
    fireEvent.contextMenu(rowOf('s1'));
    expect(screen.getByRole('menuitem')).toHaveTextContent('archive');
    await userEvent.click(screen.getByRole('menuitem'));
    expect(archiveSession).toHaveBeenCalledWith('s1');
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('right-click offers restore and delete for an archived row', async () => {
    const s = seed();
    s.archived = true;
    await mount(s);
    fireEvent.contextMenu(rowOf('s1'));
    const items = screen.getAllByRole('menuitem').map((el) => el.textContent);
    expect(items).toEqual(expect.arrayContaining(['restore', 'delete']));
    await userEvent.click(screen.getByRole('menuitem', { name: 'restore' }));
    expect(restoreSession).toHaveBeenCalledWith(WS, 's1');
  });

  it('right-clicking a selected row acts on the whole selection', async () => {
    const s2 = seedState(meta('s2'));
    const s1 = seed({ sessions: { s1: seedState(meta('s1')), s2 } });
    mockStore.pane[WS].selected = ['s1', 's2'];
    await mount(s1, { sessions: [s1, s2] });
    fireEvent.contextMenu(rowOf('s1'));
    expect(screen.getByRole('menuitem')).toHaveTextContent('archive (2)');
    await userEvent.click(screen.getByRole('menuitem'));
    expect(archiveSession).toHaveBeenCalledWith('s1');
    expect(archiveSession).toHaveBeenCalledWith('s2');
    expect(mockStore.pane[WS].selected).toEqual([]);
  });

  it('children render under an open group; the chevron toggles it', async () => {
    const parent = seedState(meta('p1'));
    const child = seedState(meta('c1', { parent: 'p1' }), { parent: 'p1' });
    seed({ sessions: { p1: parent, c1: child }, current: 'p1' });
    mockStore.pane[WS].openGroups = ['p1'];
    await mount(parent, { sessions: [parent, child] });
    expect(screen.getByText('c1')).toBeInTheDocument();
    await userEvent.click(document.querySelector('.chev')!);
    expect(mockStore.pane[WS].openGroups).toEqual([]);
    expect(screen.queryByText('c1')).toBeNull();
  });

  it('Escape closes the context menu', async () => {
    await mount(seed());
    fireEvent.contextMenu(rowOf('s1'));
    expect(screen.getByRole('menu')).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('the delete menu item deletes the row', async () => {
    const s = seed();
    s.archived = true;
    await mount(s);
    fireEvent.contextMenu(rowOf('s1'));
    await userEvent.click(screen.getByRole('menuitem', { name: 'delete' }));
    expect(deleteSession).toHaveBeenCalledWith(WS, 's1');
  });
});
