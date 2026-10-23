// The fleet list: every session across workspaces, running first.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import FleetList from './FleetList.svelte';
import { mockStore as store, switchSession, resetMockStore, seedState } from '../lib/testing/mock-store.svelte';
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
    workspaces: [{ id: 'w1', name: 'tau', cwd: '/x' }, { id: 'w2', name: 'chairs', cwd: '/y' }],
    sessions: {
      idle1: seedState(meta('idle1', { title: 'sitting' }), { state: 'idle', last_modified: 5 }),
      run1: seedState(meta('run1', { title: 'working' }), { state: 'running', last_modified: 3 }),
      run2: seedState(meta('run2', { title: 'other', workspace: 'w2' }), { state: 'running', last_modified: 9 })
    }
  });
  store.fleetOpen = true;
});

describe('FleetList', () => {
  it('renders nothing while closed', () => {
    store.fleetOpen = false;
    render(FleetList);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('lists every non-archived session with its workspace, running first', () => {
    render(FleetList);
    expect(screen.getByRole('dialog')).toBeTruthy();
    const names = screen.getAllByText(/working|sitting|other/).map((n) => n.textContent);
    // Both runners before the idle one, regardless of workspace.
    expect(names.slice(0, 2).sort()).toEqual(['other', 'working']);
    expect(names).toContain('sitting');
    // The cross-workspace item carries its workspace name.
    expect(screen.getByText('chairs')).toBeTruthy();
  });

  it('a row click jumps to the session and closes the fleet', async () => {
    const user = userEvent.setup();
    render(FleetList);
    await user.click(screen.getByText('other'));
    expect(store.fleetOpen).toBe(false);
    expect(switchSession).toHaveBeenCalledWith('run2');
  });

  it('excludes archived sessions', () => {
    resetMockStore({
      sessions: { a: seedState(meta('a', { title: 'gone', archived: true })) }
    });
    render(FleetList);
    expect(screen.queryByText('gone')).toBeNull();
  });
});
