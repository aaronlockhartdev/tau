// @vitest-environment jsdom
// The right pane: the tasks | sub-agents tabs. Tasks group into active and
// a collapsed history; a task row expands to its detail sections. The
// sub-agents panel groups live/history rows, selects on click, and opens
// the child's session on double-click.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SessionMeta, SubagentInfo, Task } from '../lib/protocol';
import type { SessionState } from '../lib/sessions';
import RightPane from './RightPane.svelte';
import {
  mockStore,
  openSessionById,
  resetMockStore,
  seedState
} from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
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

const task = (id: string, over: Partial<Task> = {}): Task => ({
  id,
  title: `task ${id}`,
  status: 'in_progress',
  steps: [],
  criteria: [],
  evidence: [],
  blockers: [],
  decisions: [],
  notes: [],
  updated: 1000,
  ...over
});

const sub = (handle: string, child: string, over: Partial<SubagentInfo> = {}): SubagentInfo => ({
  handle,
  child,
  agent_type: 'worker',
  context_mode: 'fresh',
  state: 'running',
  waiting_on: null,
  last_message: null,
  usage: null,
  task: null,
  resume_contract: null,
  ...over
});

type SeedOver = Partial<{
  current: string | null;
  sessions: Record<string, SessionState>;
  tasks: Task[];
  subagents: SubagentInfo[];
}>;

function seed(over: SeedOver = {}): void {
  const sessions: Record<string, SessionState> = {
    s1: seedState(meta('s1'), {
      over: { tasks: over.tasks ?? [], subagents: over.subagents ?? [] }
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
  it('disables the tabs without a current session', async () => {
    seed({ current: null });
    await mount();
    expect(screen.getByRole('button', { name: 'tasks' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'sub-agents' })).toBeDisabled();
  });

  it('shows the empty hint with no tasks', async () => {
    await mount();
    expect(screen.getByText('No tasks — ask the agent to make one')).toBeInTheDocument();
  });

  it('groups tasks into active and a collapsed history', async () => {
    seed({
      tasks: [task('t1'), task('t2', { status: 'done', updated: 999 })]
    });
    await mount();
    expect(screen.getByText('active · 1')).toBeInTheDocument();
    expect(screen.getByText('history · 1')).toBeInTheDocument();
    expect(screen.getByText('task t1')).toBeInTheDocument();
    expect(screen.queryByText('task t2')).toBeNull();
    await user.click(screen.getByRole('button', { name: /history · 1/ }));
    expect(mockStore.pane[WS]?.historyOpen).toBe(true);
    expect(screen.getByText('task t2')).toBeInTheDocument();
  });

  it('a task row expands to its detail sections', async () => {
    seed({
      tasks: [
        task('t1', {
          title: 'Ship the report',
          steps: [
            { text: 'draft', expected_output: '', status: 'done' },
            { text: 'review', expected_output: 'approved', status: 'active' }
          ],
          criteria: [{ text: 'report ships', status: 'satisfied' }],
          evidence: [
            {
              criterion: 'report ships',
              summary: 'shipped it',
              command: 'git push',
              passed: true
            }
          ],
          worker: { session: 's2', status: 'running' },
          updated: Date.now()
        })
      ]
    });
    await mount();
    await user.click(screen.getByRole('button', { name: /Ship the report/ }));
    expect(screen.getByText('steps — 1/2 done')).toBeInTheDocument();
    expect(screen.getByText('draft')).toBeInTheDocument();
    expect(screen.getByText('→ approved')).toBeInTheDocument();
    expect(screen.getByText('acceptance criteria')).toBeInTheDocument();
    expect(screen.getByText('evidence')).toBeInTheDocument();
    expect(screen.getByText('shipped it — git push')).toBeInTheDocument();
    expect(screen.getByText('assigned: s2 (running) · modified just now')).toBeInTheDocument();
  });

  it('a blocked task shows its blocker', async () => {
    seed({
      tasks: [
        task('t1', {
          status: 'blocked',
          blockers: [{ reason: 'waiting on api key', needs: 'user' }]
        })
      ]
    });
    await mount();
    await user.click(screen.getByRole('button', { name: /task t1/ }));
    expect(screen.getByText(/⛔ waiting on api key — user/)).toBeInTheDocument();
  });

  it('shows the empty hint with no sub-agents', async () => {
    seed({});
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    expect(screen.getByText('No sub-agents — ask the agent to spawn one')).toBeInTheDocument();
  });

  it('groups sub-agents into live and history with badge, title and task id', async () => {
    seed({
      subagents: [
        sub('h1', 'c1', { task: task('t9') }),
        sub('h2', 'c2', { state: 'done' })
      ],
      sessions: {
        c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', mru: 2 }),
        c2: seedState(meta('c2', { title: 'worker two' }), { parent: 's1', mru: 3 })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    expect(screen.getByText('live · 1')).toBeInTheDocument();
    expect(screen.getByText('history · 1')).toBeInTheDocument();
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByText('worker one')).toBeInTheDocument();
    expect(screen.getByText('t9')).toBeInTheDocument();
    expect(screen.queryByText('worker two')).toBeNull();
  });

  it('clicking a sub-agent row selects it', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', mru: 2 }) }
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
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', mru: 2 }) }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    await user.dblClick(screen.getByRole('button', { name: /worker one/ }));
    expect(openSessionById).toHaveBeenCalledWith('c1');
  });

  it('switching tabs moves the pane tab and the on marker', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: { c1: seedState(meta('c1', { title: 'worker one' }), { parent: 's1', mru: 2 }) }
    });
    await mount();
    const subsTab = screen.getByRole('button', { name: 'sub-agents' });
    expect(screen.getByRole('button', { name: 'tasks' })).toHaveClass('on');
    await user.click(subsTab);
    expect(mockStore.pane[WS]?.rtab).toBe('subs');
    expect(subsTab).toHaveClass('on');
    expect(screen.queryByText('No tasks — ask the agent to make one')).toBeNull();
  });

  it('renders a nested grandchild row under its parent', async () => {
    seed({
      subagents: [sub('h1', 'c1')],
      sessions: {
        c1: seedState(meta('c1', { title: 'worker one' }), {
          parent: 's1',
          mru: 2,
          over: { subagents: [sub('h2', 'c2')] }
        }),
        c2: seedState(meta('c2', { title: 'worker two' }), { parent: 'c1', mru: 3 })
      }
    });
    await mount();
    await user.click(screen.getByRole('button', { name: 'sub-agents' }));
    const kids = screen.getAllByText('worker two');
    expect(kids).toHaveLength(1);
    expect(kids[0]!.closest('.srow2')).toHaveClass('d2');
  });
});
