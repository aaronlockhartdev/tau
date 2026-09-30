// @vitest-environment jsdom
// The status bar: state · workspace · session name · om gauge on the left,
// the cost group (in/out/cache, tps while streaming) on the right.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import type { SessionMeta } from '../lib/protocol';
import StatusBar from './StatusBar.svelte';
import { mockStore, resetMockStore, seedState } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const WS = { id: 'w1', name: 'proj', cwd: '/p' };
function meta(id: string, over: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id,
    workspace: WS.id,
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

beforeEach(() => {
  resetMockStore();
});

function openSession(sid: string, over: Parameters<typeof seedState>[1] = {}) {
  mockStore.sessions = { [sid]: seedState(meta(sid), over) };
  mockStore.current = sid;
}

describe('StatusBar', () => {
  it('shows the empty state with no current session', () => {
    render(StatusBar);
    expect(screen.getByText('idle')).toBeInTheDocument();
    expect(screen.getAllByText('—')).toHaveLength(2);
    expect(screen.getByText('0 in')).toBeInTheDocument();
    expect(screen.getByText('0 out')).toBeInTheDocument();
    expect(screen.getByText('0% cache')).toBeInTheDocument();
  });

  it('shows the workspace and session title, and the idle om gauge', () => {
    openSession('s1', {
      over: {
        om: { kind: 'idle', observation_tokens: 1200, pending_tokens: 0, reflector_threshold: 1500 }
      }
    });
    mockStore.sessions['s1'].meta = meta('s1', { title: 'alpha' });
    mockStore.workspaces = [WS];
    render(StatusBar);
    expect(screen.getByText('proj')).toBeInTheDocument();
    expect(screen.getByText('alpha')).toBeInTheDocument();
    expect(screen.getByText('1.2k/1.5k')).toBeInTheDocument();
  });

  it('a running turn shows the running tag and tps', () => {
    openSession('s1', { over: { turn: 'running', tps: 42 } });
    render(StatusBar);
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByText('42 tps')).toBeInTheDocument();
  });

  it('a starting turn reads as running', () => {
    openSession('s1', { over: { turn: 'starting' } });
    render(StatusBar);
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.queryByText('tps')).toBeNull();
  });

  it('a busy om kind shows the activity word instead of the gauge', () => {
    openSession('s1', {
      over: {
        om: { kind: 'observing', observation_tokens: 10, pending_tokens: 5, reflector_threshold: 100 }
      }
    });
    render(StatusBar);
    expect(screen.getByText('observing')).toBeInTheDocument();
    expect(screen.queryByText('10/100')).toBeNull();
  });

  it('usage renders the cost group with the cache percentage', () => {
    openSession('s1', {
      over: {
        usage: {
          input_tokens: 1200,
          output_tokens: 340,
          cached_prompt_tokens: 600,
          total_tokens: 1540
        }
      }
    });
    render(StatusBar);
    expect(screen.getByText('1.2k in')).toBeInTheDocument();
    expect(screen.getByText('340 out')).toBeInTheDocument();
    expect(screen.getByText('50% cache')).toBeInTheDocument();
  });

  it('a workspace missing from the tab strip falls back to the id', () => {
    openSession('s1');
    render(StatusBar);
    expect(screen.getByText('w1')).toBeInTheDocument();
  });
});
