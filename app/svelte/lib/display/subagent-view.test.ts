// The crew correlation (P2): spawn points in the parent transcript claimed
// to live SubagentInfo, and the block's display view across the lifecycle.

import { describe, expect, it } from 'vitest';
import type { SubagentInfo } from '../wire/protocol';
import { claimCrew, crewView, spawnRefsOf, type SpawnRef } from './subagent-view';

function spawn(over: Partial<SpawnRef> = {}): SpawnRef {
  return {
    entryId: 't1',
    agentType: 'general',
    contextMode: 'fresh',
    brief: 'the brief',
    ...over
  };
}

function info(over: Partial<SubagentInfo> = {}): SubagentInfo {
  return {
    handle: 'h1',
    child: 'child-1',
    agent_type: 'general',
    context_mode: 'fresh',
    state: 'running',
    waiting_on: null,
    last_message: null,
    usage: null,
    resume_contract: null,
    ...over
  };
}

describe('spawnRefsOf', () => {
  it('collects the subagent_spawn tool entries in order', () => {
    const refs = spawnRefsOf([
      { id: 'a', kind: 'tool', name: 'bash', args: { command: 'ls' } },
      { id: 'b', kind: 'tool', name: 'subagent_spawn', args: { type: 'general', brief: 'one' } },
      { id: 'c', kind: 'message', text: 'hi' },
      {
        id: 'd',
        kind: 'tool',
        name: 'subagent_spawn',
        args: { type: 'reviewer', brief: 'two', context_mode: 'compacted' }
      }
    ]);
    expect(refs).toEqual([
      { entryId: 'b', agentType: 'general', contextMode: 'fresh', brief: 'one' },
      { entryId: 'd', agentType: 'reviewer', contextMode: 'compacted', brief: 'two' }
    ]);
  });

  it('ignores a spawn entry without args (the metadata skeleton)', () => {
    expect(spawnRefsOf([{ id: 'b', kind: 'tool', name: 'subagent_spawn' }])).toEqual([]);
  });
});

describe('claimCrew', () => {
  it('matches distinct types', () => {
    const refs = [spawn({ entryId: 'a' }), spawn({ entryId: 'b', agentType: 'reviewer' })];
    const m = claimCrew(refs, [info({ handle: 'h1' }), info({ handle: 'h2', agent_type: 'reviewer' })]);
    expect(m.get('a')?.handle).toBe('h1');
    expect(m.get('b')?.handle).toBe('h2');
  });

  it('breaks multiplicity ties in spawn order', () => {
    const refs = [spawn({ entryId: 'a' }), spawn({ entryId: 'b' })];
    const m = claimCrew(refs, [info({ handle: 'h1' }), info({ handle: 'h2' })]);
    expect(m.get('a')?.handle).toBe('h1');
    expect(m.get('b')?.handle).toBe('h2');
  });

  it('leaves a spawn unclaimed when the crew is cold', () => {
    const m = claimCrew([spawn()], []);
    expect(m.get('t1')).toBeNull();
  });

  it('leaves the extra spawn unclaimed when the crew is shorter', () => {
    const refs = [spawn({ entryId: 'a' }), spawn({ entryId: 'b' })];
    const m = claimCrew(refs, [info()]);
    expect(m.get('a')?.handle).toBe('h1');
    expect(m.get('b')).toBeNull();
  });
});

describe('crewView', () => {
  const ref = spawn();

  it('drives the block through the lifecycle: spawn → running → waiting → done', () => {
    // Cold session: the static spawn marker.
    expect(crewView(ref, null).phase).toBe('spawn');

    // Spawned and running, with a live line and tokens.
    const running = crewView(
      ref,
      info({ last_message: 'reading the file', usage: { input_tokens: 1200, output_tokens: 80, total_tokens: 1280, cached_prompt_tokens: 0 } })
    );
    expect(running).toEqual({
      phase: 'running',
      agentType: 'general',
      contextMode: 'fresh',
      brief: 'the brief',
      lastMessage: 'reading the file',
      inputTokens: 1200,
      outputTokens: 80,
      waitingOn: null,
      handle: 'h1',
      child: 'child-1'
    });

    // Parked on the user: the red "needs you" case.
    expect(crewView(ref, info({ state: 'idle', waiting_on: 'user' })).phase).toBe('waiting-user');

    // Parked on the parent: a quiet wait, not red.
    expect(crewView(ref, info({ state: 'idle', waiting_on: 'parent' })).phase).toBe(
      'waiting-parent'
    );

    // Done: the report is the parent_notify text.
    const done = crewView(ref, info({ state: 'done', last_message: 'report: all green' }));
    expect(done.phase).toBe('done');
    expect(done.lastMessage).toBe('report: all green');

    // The terminal failures.
    expect(crewView(ref, info({ state: 'failed' })).phase).toBe('failed');
    expect(crewView(ref, info({ state: 'stopped' })).phase).toBe('stopped');
  });

  it('keeps null tokens for a running child without usage yet', () => {
    const v = crewView(ref, info());
    expect(v.inputTokens).toBeNull();
    expect(v.outputTokens).toBeNull();
  });
});
