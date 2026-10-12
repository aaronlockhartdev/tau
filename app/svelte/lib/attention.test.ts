// The attention fold: a pure projection over session state.
import { describe, expect, it } from 'vitest';
import { attentionOf } from './attention';
import { seedState } from './testing/mock-store.svelte.ts';
import type { SessionMeta, SubagentInfo, Task } from './protocol';

function meta(id: string, over: Partial<SessionMeta> = {}): SessionMeta {
  return {
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
  };
}

const sub = (over: Partial<SubagentInfo> = {}): SubagentInfo => ({
  handle: 'h1',
  child: 'c1',
  agent_type: 'researcher',
  context_mode: 'fresh',
  state: 'idle',
  waiting_on: 'user',
  last_message: null,
  usage: null,
  task: null,
  resume_contract: null,
  ...over
});

const task = (over: Partial<Task> = {}): Task => ({
  id: 't1',
  title: 'Ship the report',
  status: 'blocked',
  steps: [],
  criteria: [],
  evidence: [],
  blockers: [{ reason: 'needs api key', needs: 'user' }],
  decisions: [],
  notes: [],
  updated: 5000,
  ...over
});

describe('attentionOf', () => {
  it('finds a child parked on the user', () => {
    const items = attentionOf({
      s1: seedState(meta('s1', { title: 'hero' }), {
        over: { subagents: [sub()] }
      })
    });
    expect(items).toHaveLength(1);
    expect(items[0]!.session).toBe('s1');
    expect(items[0]!.reason).toBe('researcher is waiting on you');
  });

  it('names the child from its session title when known', () => {
    const items = attentionOf({
      s1: seedState(meta('s1'), { over: { subagents: [sub()] } }),
      c1: seedState(meta('c1', { title: 'the digger' }), { parent: 's1' })
    });
    expect(items[0]!.reason).toBe('the digger is waiting on you');
  });

  it('finds a task blocked on the user, with the blocker reason', () => {
    const items = attentionOf({
      s1: seedState(meta('s1'), { over: { tasks: [task()] } })
    });
    expect(items).toHaveLength(1);
    expect(items[0]!.reason).toBe('Ship the report — needs api key');
    expect(items[0]!.since).toBe(5000);
  });

  it('ignores non-blocking signals', () => {
    const items = attentionOf({
      s1: seedState(meta('s1'), {
        over: {
          subagents: [
            sub({ state: 'running' }),
            sub({ waiting_on: 'parent' }),
            sub({ state: 'done', waiting_on: null })
          ],
          tasks: [
            task({ status: 'in_progress' }),
            task({ blockers: [{ reason: 'stuck on ci' }] }),
            task({ status: 'done' })
          ]
        }
      })
    });
    expect(items).toHaveLength(0);
  });

  it('folds across workspaces and orders by most recent', () => {
    const items = attentionOf({
      a: seedState(meta('a', { workspace: 'w1', title: 'one' }), {
        over: { tasks: [task({ updated: 5000 })] }
      }),
      b: seedState(meta('b', { workspace: 'w2', title: 'two' }), {
        over: { tasks: [task({ id: 't2', title: 'Other', updated: 9000 })] }
      }),
      c: seedState(meta('c', { workspace: 'w2', title: 'three' }), {
        over: { subagents: [sub({ handle: 'h9' })] }
      }),
      d: seedState(meta('d', { archived: true }), {
        over: { tasks: [task()] }
      })
    });
    // The archived session is out; the cross-workspace pair is in.
    expect(items.map((i) => i.session)).toEqual(['b', 'a', 'c']);
    expect(items[0]!.workspace).toBe('w2');
  });
});
