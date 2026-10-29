// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import type { Plan } from '../wire/protocol';
import { fleetGate, planProgress } from './plan';

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
    expect(fleetGate('done')).toBe('pass');
    expect(fleetGate('failed')).toBe('fail');
  });

  it('a live child gate is pending', () => {
    expect(fleetGate('running')).toBe('pending');
    expect(fleetGate('idle')).toBe('pending');
    expect(fleetGate('stopped')).toBe('pending');
  });
});
