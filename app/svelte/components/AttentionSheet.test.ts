// The attention sheet: the bell's overlay.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import AttentionSheet from './AttentionSheet.svelte';
import { mockStore as store, switchSession, resetMockStore, seedState } from '../lib/testing/mock-store.svelte';
import type { SessionMeta } from '../lib/wire/protocol';

vi.mock('../lib/store/store.svelte', async () => {
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
    sessions: { s1: seedState(meta('s1', { title: 'hero' })) }
  });
});

describe('AttentionSheet', () => {
  it('renders nothing while closed', () => {
    render(AttentionSheet);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('lists the needs-you rows with reason and workspace', () => {
    resetMockStore({
      sessions: {
        s1: seedState(meta('s1', { title: 'hero' }), {
          over: {
            subagents: [
              {
                handle: 'h1',
                child: 'c1',
                agent_type: 'researcher',
                context_mode: 'fresh',
                state: 'idle',
                waiting_on: 'user',
                last_message: null,
                usage: null,
                resume_contract: null
              }
            ]
          }
        })
      },
      workspaces: [{ id: 'w1', name: 'tau', cwd: '/x' }]
    });
    store.attentionOpen = true;
    render(AttentionSheet);
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(screen.getByText('hero')).toBeTruthy();
    expect(screen.getByText('researcher is waiting on you')).toBeTruthy();
  });

  it('jump opens the session and closes the sheet', async () => {
    const user = userEvent.setup();
    resetMockStore({
      sessions: {
        s1: seedState(meta('s1', { title: 'hero' }), {
          over: {
            subagents: [
              {
                handle: 'h1',
                child: 'c1',
                agent_type: 'researcher',
                context_mode: 'fresh',
                state: 'idle',
                waiting_on: 'user',
                last_message: null,
                usage: null,
                resume_contract: null
              }
            ]
          }
        })
      }
    });
    store.attentionOpen = true;
    render(AttentionSheet);
    await user.click(screen.getByText('hero'));
    expect(store.attentionOpen).toBe(false);
    expect(switchSession).toHaveBeenCalledWith('s1');
  });
});
