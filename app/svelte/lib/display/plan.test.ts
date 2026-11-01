// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import type { Plan } from '../wire/protocol';
import { fleetGate, planProgress } from './plan';
import { st } from '../testing/state';
import { stateOfBadge } from './subagent-state';

const plan = (over: Partial<Plan> = {}): Plan => ({
  steps: [],
  criteria: '',
  notes: '',
  ...over
});

describe('planProgress', () => {
  it('projects N/M and the first open item as current', () => {
    const p = planProgress(
      plan({
        steps: [
          { text: 'investigate', done: true },
          { text: 'fix the parser', done: false },
          { text: 'add tests', done: false }
        ]
      })
    );
    expect(p).toEqual({ done: 1, total: 3, current: 'fix the parser' });
  });

  it('an empty plan is 0/0 with no current step', () => {
    expect(planProgress(plan())).toEqual({ done: 0, total: 0, current: null });
  });

  it('a complete plan has no current step', () => {
    const p = planProgress(
      plan({ steps: [{ text: 'done', done: true }, { text: 'also done', done: true }] })
    );
    expect(p).toEqual({ done: 2, total: 2, current: null });
  });
});

describe('fleetGate', () => {
  it('settles from the observed lifecycle: done passes, failed fails', () => {
    expect(fleetGate(st({ state: 'done' }))).toBe('pass');
    expect(fleetGate(st({ state: 'failed' }))).toBe('fail');
  });

  it('a live child gate is pending', () => {
    expect(fleetGate(st({ state: 'running' }))).toBe('pending');
    expect(fleetGate(st({ state: 'idle' }))).toBe('pending');
    expect(fleetGate(st({ state: 'stopped' }))).toBe('pending');
  });

  it('a restored tab spells the state, so its verdict rides the word', () => {
    // The tree row of a child the core no longer lists: the badge word, no arm.
    expect(fleetGate(stateOfBadge('failed', null))).toBe('fail');
    expect(fleetGate(stateOfBadge('done', null))).toBe('pass');
    // A name this build does not know states no verdict (the decoder's arm).
    expect(fleetGate({ type: 'other', raw: 'parked' })).toBe('pending');
  });
});
