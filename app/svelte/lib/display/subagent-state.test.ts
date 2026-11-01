import { describe, expect, it } from 'vitest';
import {
  badgeLabel,
  badgeOf,
  badgeOfWake,
  isWaitingOnUser,
  stateOfBadge,
  stateOfWake,
  waitingHintOf,
  waitingOnOf
} from './subagent-state';

describe('the tag is the source of the badge and its detail', () => {
  it('reads each arm as its badge, and the same arm for the detail', () => {
    expect(badgeOf({ type: 'running' })).toBe('running');
    expect(badgeOf({ type: 'awaiting' })).toBe('awaiting');
    expect(badgeOf({ type: 'done', output: null })).toBe('done');
    expect(badgeOf({ type: 'failed', reason: 'the reason' })).toBe('failed');
    expect(waitingOnOf({ type: 'idle', waiting_on: 'user' })).toBe('user');
    expect(waitingOnOf({ type: 'running' })).toBeNull();
  });

  it('reads the spelling the wire sent for a state this build does not know', () => {
    expect(badgeOf({ type: 'other', raw: 'parked' })).toBe('other');
    expect(badgeLabel({ type: 'other', raw: 'parked' })).toBe('parked');
    expect(badgeLabel({ type: 'other', raw: '' })).toBe('unknown');
    expect(badgeLabel({ type: 'idle', waiting_on: 'user' })).toBe('idle');
  });

  it('reads a tree row as the state its badge and target spell', () => {
    expect(stateOfBadge('running', null)).toEqual({ type: 'running' });
    expect(stateOfBadge('awaiting', null)).toEqual({ type: 'awaiting' });
    // A row's state is a value, so an idle with no declared target is the
    // delegating parent's.
    expect(stateOfBadge('idle', null)).toEqual({ type: 'idle', waiting_on: 'parent' });
    expect(stateOfBadge('idle', 'subagent')).toEqual({ type: 'idle', waiting_on: 'subagent' });
    // A badge that states no detail invents none: the arm keeps the
    // spelling and names no reason, no storer.
    expect(stateOfBadge('done', 'user')).toEqual({ type: 'done', output: null });
    expect(stateOfBadge('failed', 'user')).toEqual({ type: 'other', raw: 'failed' });
    expect(stateOfBadge('stopped', 'user')).toEqual({ type: 'other', raw: 'stopped' });
  });
});

describe('a wake settles the row', () => {
  it('maps each wake to the badge it reports', () => {
    expect(badgeOfWake('done')).toBe('done');
    expect(badgeOfWake('failed')).toBe('failed');
    expect(badgeOfWake('waiting')).toBe('idle');
    // A stopped wake says the child stopped; the mirror's tag keeps the
    // `other` arm, because that arm requires a storer the wake never names.
    expect(badgeOfWake('stopped')).toBe('stopped');
    expect(badgeOfWake('nonsense')).toBe('other');
  });

  it('carries the report and the reason the wake states', () => {
    expect(stateOfWake('done', 'a report', { result: 'ok' }, 'user')).toEqual({
      type: 'done',
      output: { result: 'ok' }
    });
    expect(stateOfWake('failed', 'the reason', null, null)).toEqual({
      type: 'failed',
      reason: 'the reason'
    });
  });

  it('keeps the target the row declares, for a wake states none', () => {
    // A wake re-parks a child where it was: the GUI reads `waiting · user`,
    // never the invented `waiting · parent` (spec §5.2).
    expect(stateOfWake('waiting', 'needs input', null, 'user')).toEqual({
      type: 'idle',
      waiting_on: 'user'
    });
  });

  it('parks a row with no observed wait on the delegating parent', () => {
    expect(stateOfWake('waiting', 'needs input', null, null)).toEqual({
      type: 'idle',
      waiting_on: 'parent'
    });
  });

  it('invents no storer for a stopped wake', () => {
    expect(stateOfWake('stopped', 'stopped', null, null)).toEqual({
      type: 'other',
      raw: 'stopped'
    });
  });
});

describe('the waiting-for-user question is one rule', () => {
  it('is the awaiting arm, or an idle arm that declares the user', () => {
    expect(isWaitingOnUser({ type: 'awaiting' })).toBe(true);
    expect(isWaitingOnUser({ type: 'idle', waiting_on: 'user' })).toBe(true);
    expect(isWaitingOnUser({ type: 'idle', waiting_on: 'parent' })).toBe(false);
    expect(isWaitingOnUser({ type: 'running' })).toBe(false);
  });

  it('hints the badge with the arm’s own fact', () => {
    expect(waitingHintOf({ type: 'awaiting' })).toBe('user');
    expect(waitingHintOf({ type: 'idle', waiting_on: 'subagent' })).toBe('subagent');
    expect(waitingHintOf({ type: 'running' })).toBeNull();
  });
});
