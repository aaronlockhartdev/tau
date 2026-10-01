// @vitest-environment jsdom
// The queue above the composer: sub-agent reports on top, then steering
// ("next opportunity"), then follow-up ("after work completes"); each
// deletable row issues deleteQueueItem with its section index.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta } from '../lib/protocol';
import Queue from './Queue.svelte';
import { deleteQueueItem, mockStore, resetMockStore, seedState } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const WS = { id: 'w1', name: 'proj', cwd: '/p' };
function meta(id: string): SessionMeta {
  return {
    id,
    workspace: WS.id,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false
  };
}

beforeEach(() => {
  resetMockStore();
});

describe('Queue', () => {
  it('renders nothing with no current session', () => {
    render(Queue);
    expect(screen.queryAllByText(/opportunity/)).toHaveLength(0);
  });

  it('renders nothing for an empty queue', () => {
    mockStore.sessions = { s1: seedState(meta('s1')) };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('lists steering items under "next opportunity"', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: { pending: [{ text: 'steer me', lane: 'steering' }] }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.getByText('next opportunity')).toBeInTheDocument();
    expect(screen.getByText('steer me')).toBeInTheDocument();
  });

  it('lists follow-up items under "after work completes"', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: { pending: [{ text: 'then check', lane: 'follow-up' }] }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.getByText('after work completes')).toBeInTheDocument();
    expect(screen.getByText('then check')).toBeInTheDocument();
  });

  it('a sub-agent report gets the reports section and its source tail', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: { pending: [{ text: 'report', lane: 'steering', source: 's1-2' }] }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.getByText('subagent reports')).toBeInTheDocument();
    expect(screen.getByText('sub-agent 2')).toBeInTheDocument();
    // a sourced item is not deletable from the queue
    expect(screen.queryAllByRole('button')).toHaveLength(0);
  });

  it('delete issues deleteQueueItem with the item text, lane and section index', async () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: {
          pending: [
            { text: 'first', lane: 'steering' },
            { text: 'second', lane: 'steering' },
            { text: 'later', lane: 'follow-up' }
          ]
        }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    const user = userEvent.setup();
    const dels = screen.getAllByRole('button');
    await user.click(dels[1]!);
    expect(deleteQueueItem).toHaveBeenCalledWith('second', 'steering', null, 1);
    // the mock keeps the rows; the last button is the follow-up section's (index 0 there)
    await user.click(screen.getAllByRole('button').at(-1)!);
    expect(deleteQueueItem).toHaveBeenCalledWith('later', 'follow-up', null, 0);
  });

  it('sections render in reports / steering / follow-up order', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: {
          pending: [
            { text: 'fu', lane: 'follow-up' },
            { text: 'rep', lane: 'follow-up', source: 'p-1' },
            { text: 'st', lane: 'steering' }
          ]
        }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    const labels = screen
      .getAllByText(/reports|opportunity|completes/)
      .map((el) => el.textContent);
    expect(labels).toEqual(['subagent reports', 'next opportunity', 'after work completes']);
  });
});
