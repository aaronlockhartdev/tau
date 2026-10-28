// @vitest-environment jsdom
// The right pane: the sub-agents panel groups live/history rows, selects
// on click, and opens the child's session on double-click.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta, SubagentInfo } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import RightPane from './RightPane.svelte';
import {
  mockStore,
  switchSession,
  resetMockStore,
  seedState
} from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
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

const sub = (handle: string, child: string, over: Partial<SubagentInfo> = {}): SubagentInfo => ({
  handle,
  child,
  agent_type: 'worker',
  context_mode: 'fresh',
  state: 'running',
  waiting_on: null,
  last_message: null,
  usage: null,
  resume_contract: null,
  ...over
});

type SeedOver = Partial<{
  current: string | null;
  sessions: Record<string, SessionState>;
  subagents: SubagentInfo[];
}>;

function seed(over: SeedOver = {}): void {
  const sessions: Record<string, SessionState> = {
    s1: seedState(meta('s1'), {
      over: { subagents: over.subagents ?? [] }
    }),
    ...(over.sessions ?? {})
  };
  resetMockStore({
    current: over.current === undefined ? 's1' : over.current,
    sessions
  });
}

async function mount(): Promise<void> {
  render(RightPane);
  // The ensurePane effect creates the pane after the first render.
  await tick();
}

beforeEach(() => {
  seed();
});

const user = userEvent.setup();

describe('RightPane', () => {
  it('disables the tab without a current session', async () => {
    seed({ current: null });
    await mount();
    expect(screen.getByRole('button', { name: 'sub-agents' })).toBeDisabled();
  });

  it('shows the empty hint with no sub-agents', async () => {
    seed({});
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    expect(screen.getByText('No sub-agents — ask the agent to spawn one')).toBeInTheDocument();
  });

  it('groups sub-agents into live and history with badge, title, chips and live line', async () => {
    seed({
      subagents: [
        sub('h1', 'c1', {
          last_message: 'reading the config files',
          usage: { input_tokens: 1200, output_tokens: 80, total_tokens: 1280, cached_prompt_tokens: 0 }
        }),
        sub('h2', 'c2', { state: 'done' })
      ],
      sessions: {
        c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }),
        c2: seedState(meta('c2', { title: 'worker two' }), { parent: 's1', last_modified: 3 })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    expect(screen.getByText('live · 1')).toBeInTheDocument();
    expect(screen.getByText('history · 1')).toBeInTheDocument();
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByText('worker one')).toBeInTheDocument();
    expect(screen.getByText('1.2k in · 80 out')).toBeInTheDocument();
    expect(screen.getByText('reading the config files')).toBeInTheDocument();
    expect(screen.queryByText('worker two')).toBeNull();
  });

  it('orders the crew running first, then waiting, then terminal', async () => {
    seed({
      subagents: [
        sub('hA', 'cA', { state: 'idle', waiting_on: 'parent' }),
        sub('hB', 'cB'),
        sub('hC', 'cC', { state: 'done' }),
        sub('hD', 'cD', { state: 'idle', waiting_on: 'user' })
      ],
      sessions: {
        cA: seedState(meta('cA', { title: 'alpha' }), { parent: 's1', last_modified: 1 }),
        cB: seedState(meta('cB', { title: 'beta' }), { parent: 's1', last_modified: 2 }),
        cC: seedState(meta('cC', { title: 'gamma' }), { parent: 's1', last_modified: 3 }),
        cD: seedState(meta('cD', { title: 'delta' }), { parent: 's1', last_modified: 4 })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    await user.click(screen.getByRole('button', { name: /history · 1/ }));
    const names = [...document.querySelectorAll('.crewrow .ln')].map((n) => n.textContent ?? '');
    expect(names).toEqual(['beta', 'delta', 'alpha', 'gamma']);
  });

  it('refreshes the live line when a new last_message lands', async () => {
    seed({
      subagents: [sub('h1', 'c1', { last_message: 'first line' })],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    expect(screen.getByText('first line')).toBeInTheDocument();
    mockStore.sessions['s1'] = seedState(meta('s1'), {
      over: { subagents: [sub('h1', 'c1', { last_message: 'second line' })] }
    });
    await tick();
    expect(screen.getByText('second line')).toBeInTheDocument();
  });

  it('follow toggles the store follow handle', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    await user.click(screen.getByRole('button', { name: 'follow' }));
    expect(mockStore.follow).toBe('h1');
    await user.click(screen.getByRole('button', { name: 'following' }));
    expect(mockStore.follow).toBeNull();
  });

  it('open calls switchSession with the child id', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    await user.click(screen.getByRole('button', { name: 'open' }));
    expect(switchSession).toHaveBeenCalledWith('c1');
  });

  it('clicking a sub-agent row selects it', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    const row = screen.getByRole('button', { name: /worker one/ });
    await user.click(row);
    expect(mockStore.pane[WS]?.selSub).toBe('h1');
    expect(row).toHaveClass('sel');
  });

  it('double-clicking a sub-agent row opens the child session', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    await user.dblClick(screen.getByRole('button', { name: /worker one/ }));
    expect(switchSession).toHaveBeenCalledWith('c1');
  });

  it('renders a nested grandchild row under its parent', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: {
        c1: seedState(meta('c1', { title: 'worker one' }), {
          parent: 's1',
          last_modified: 2,
          over: { subagents: [sub('h2', 'c2')] }
        }),
        c2: seedState(meta('c2', { title: 'worker two' }), { parent: 'c1', last_modified: 3 })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    const kids = screen.getAllByText('worker two');
    expect(kids).toHaveLength(1);
    expect(kids[0]!.closest('.srow2')).toHaveClass('d2');
  });

  it('the plan tab shows the session plan: steps, N/M, current step, gate', async () => {
    seed({
      sessions: {
        s1: seedState(meta('s1'), {
          over: {
            plan: {
              steps: [
                { text: 'investigate', done: true },
                { text: 'fix the parser', done: false }
              ],
              criteria: 'build passes',
              notes: ''
            },
            gate_failed: true
          }
        })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'plan' }));
    expect(screen.getByText('1/2')).toBeInTheDocument();
    expect(screen.getByText('investigate')).toBeInTheDocument();
    expect(screen.getByText('fix the parser')).toBeInTheDocument();
    expect(screen.getByText('gate failed')).toBeInTheDocument();
  });

  it('the plan tab shows the empty state with no plan', async () => {
    seed({});
    await mount();
    await user.click(screen.getByRole('button', { name: 'plan' }));
    expect(screen.getByText('No plan yet — the agent writes it as it works')).toBeInTheDocument();
  });

  it('sub-agent rows carry the fleet projection: N/M, current step, gate', async () => {
    seed({
      subagents: [sub('h1', 'c1'), sub('h2', 'c2', { state: 'done' })],
      sessions: {
        c1: seedState(meta('c1', { title: 'worker one' }), {
          parent: 's1',
          last_modified: 2,
          over: {
            plan: {
              steps: [
                { text: 'read the spec', done: true },
                { text: 'write the fix', done: false }
              ],
              criteria: '',
              notes: ''
            }
          }
        }),
        c2: seedState(meta('c2', { title: 'worker two' }), {
          parent: 's1',
          last_modified: 3,
          over: {
            plan: { steps: [{ text: 'the work', done: true }], criteria: '', notes: '' }
          }
        })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    // The running child: progress + current step + the gate still pending.
    expect(screen.getByText('1/2 · write the fix · gate …')).toBeInTheDocument();
    // The done child (history): the gate verdict is settled — pass.
    await user.click(screen.getByRole('button', { name: /history · 1/ }));
    expect(screen.getByText('1/1 · gate ✓')).toBeInTheDocument();
  });

  it('a child without a plan shows no projection line', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', last_modified: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    expect(document.querySelector('.l3')).toBeNull();
  });
});
