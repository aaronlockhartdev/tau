// @vitest-environment jsdom
// The queue above the composer: sub-agent reports on top, then steering
// ("next tool boundary"), then follow-up ("after work completes"); each
// deletable row issues deleteQueueItem with its section index.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta } from '../lib/wire/protocol';
import Queue from './Queue.svelte';
import { deleteQueueItem, mockStore, resetMockStore, seedState } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store/store.svelte', async () => {
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

  it('lists steering items under "next tool boundary"', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: { pending: [{ id: 'q1', text: 'steer me', lane: 'steering' }] }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.getByText('next tool boundary')).toBeInTheDocument();
    expect(screen.getByText('steer me')).toBeInTheDocument();
  });

  it('lists follow-up items under "after work completes"', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: { pending: [{ id: 'q1', text: 'then check', lane: 'follow-up' }] }
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
        over: { pending: [{ id: 'q1', text: 'report', lane: 'steering', source: 's1-2' }] }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.getByText('subagent reports')).toBeInTheDocument();
    expect(screen.getByText('sub-agent 2')).toBeInTheDocument();
    // a sourced item is not deletable from the queue
    expect(screen.queryAllByRole('button')).toHaveLength(0);
  });

  it('a user-sourced item stays in the plain steering section', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: {
          pending: [
            { id: 'q1', text: 'steer', lane: 'steering', source: 'user' },
            { id: 'q2', text: 'report', lane: 'steering', source: 's1-2' }
          ]
        }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    // The human's send is deletable (a plain row); the sub-agent's is not.
    expect(screen.getByText('steer')).toBeInTheDocument();
    expect(screen.getByText('subagent reports')).toBeInTheDocument();
    expect(screen.getAllByRole('button')).toHaveLength(1);
  });

  it('delete issues deleteQueueItem with the row id the core issued', async () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: {
          pending: [
            { id: 'q1', text: 'first', lane: 'steering' },
            { id: 'q2', text: 'second', lane: 'steering' },
            { id: 'q3', text: 'later', lane: 'follow-up' }
          ]
        }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    const user = userEvent.setup();
    const dels = screen.getAllByRole('button');
    await user.click(dels[1]!);
    expect(deleteQueueItem).toHaveBeenCalledWith('q2');
    // the mock keeps the rows; the last button is the follow-up section's row
    await user.click(screen.getAllByRole('button').at(-1)!);
    expect(deleteQueueItem).toHaveBeenCalledWith('q3');
  });

  it('the queue of a child session renders no delete control', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        parent: 'p1',
        over: { pending: [{ id: 'q1', text: 'steer', lane: 'steering' }] }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    expect(screen.getByText('steer')).toBeInTheDocument();
    expect(screen.queryAllByRole('button')).toHaveLength(0);
  });

  it('sections render in reports / steering / follow-up order', () => {
    mockStore.sessions = {
      s1: seedState(meta('s1'), {
        over: {
          pending: [
            { id: 'q1', text: 'fu', lane: 'follow-up' },
            { id: 'q2', text: 'rep', lane: 'follow-up', source: 'p-1' },
            { id: 'q3', text: 'st', lane: 'steering' }
          ]
        }
      })
    };
    mockStore.current = 's1';
    render(Queue);
    const labels = screen
      .getAllByText(/reports|boundary|completes/)
      .map((el) => el.textContent);
    expect(labels).toEqual(['subagent reports', 'next tool boundary', 'after work completes']);
  });
});
