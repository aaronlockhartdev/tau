// The windowing policy (F1), unit-tested directly — the interface is the test
// surface. Positive per fetch trigger (hysteresis drift, open-tail, turn-end
// re-read, mid-turn union) and the negatives (inside-margin no-op, not-idle
// no-op, empty-union no-op).
import { describe, expect, it } from 'vitest';
import {
  FETCH_MARGIN,
  OPEN_TAIL,
  makeWindowingState,
  openFetches,
  scrollFetches,
  turnEndFetches
} from './windowing';

describe('scrollFetches (hysteresis + mid-turn union)', () => {
  it('fetches the visible range when no page has been fetched yet', () => {
    const st = makeWindowingState();
    expect(scrollFetches(st, 'c', { start: 0, end: 0 }, 'idle')).toEqual([
      { session: 'c', start: 0, count: 0 }
    ]);
    expect(st.lastFetched.get('c')).toEqual({ start: 0, end: 0 });
  });

  it('fetches when the range drifts beyond the margin from the last fetch', () => {
    const st = makeWindowingState();
    st.lastFetched.set('c', { start: 10, end: 30 });
    // start 0 is more than FETCH_MARGIN below last.start 10.
    expect(scrollFetches(st, 'c', { start: 0, end: 0 }, 'idle')).toEqual([
      { session: 'c', start: 0, count: 0 }
    ]);
  });

  it('is a no-op while the range is inside the hysteresis margin', () => {
    const st = makeWindowingState();
    st.lastFetched.set('c', { start: 10, end: 30 });
    // start 8 >= 10 - 5 and end 32 <= 30 + 5: inside the margin.
    expect(scrollFetches(st, 'c', { start: 10 - FETCH_MARGIN + 2, end: 30 + FETCH_MARGIN - 2 }, 'idle')).toEqual([]);
    // the last fetch is untouched by a no-op
    expect(st.lastFetched.get('c')).toEqual({ start: 10, end: 30 });
  });

  it('treats the margin boundary as inside (inclusive)', () => {
    const st = makeWindowingState();
    st.lastFetched.set('c', { start: 10, end: 30 });
    // exactly at the margin on both sides: still inside.
    expect(
      scrollFetches(st, 'c', { start: 10 - FETCH_MARGIN, end: 30 + FETCH_MARGIN }, 'idle')
    ).toEqual([]);
  });

  it('fetches one step past the margin boundary', () => {
    const st = makeWindowingState();
    st.lastFetched.set('c', { start: 10, end: 30 });
    expect(
      scrollFetches(st, 'c', { start: 10 - FETCH_MARGIN - 1, end: 30 }, 'idle')
    ).toEqual([{ session: 'c', start: 10 - FETCH_MARGIN - 1, count: 30 - (10 - FETCH_MARGIN - 1) }]);
  });

  it('unions the fetched range into the mid-turn set while a turn runs', () => {
    const st = makeWindowingState();
    scrollFetches(st, 'c', { start: 0, end: 0 }, 'running');
    scrollFetches(st, 'c', { start: 10, end: 20 }, 'running');
    expect(st.duringTurn.get('c')).toEqual({ start: 0, end: 20 });
  });

  it('does not union while the turn is idle', () => {
    const st = makeWindowingState();
    scrollFetches(st, 'c', { start: 0, end: 0 }, 'idle');
    expect(st.duringTurn.get('c')).toBeUndefined();
  });
});

describe('openFetches (open-tail + mid-turn union)', () => {
  it('fetches the tail: the last OPEN_TAIL entries', () => {
    const st = makeWindowingState();
    expect(openFetches(st, 'c', 30, 'idle')).toEqual([{ session: 'c', start: 10, count: 20 }]);
    expect(st.lastFetched.get('c')).toEqual({ start: 10, end: 30 });
  });

  it('fetches the whole list when it is shorter than the tail', () => {
    const st = makeWindowingState();
    expect(openFetches(st, 'c', 5, 'idle')).toEqual([{ session: 'c', start: 0, count: 5 }]);
    expect(st.lastFetched.get('c')).toEqual({ start: 0, end: 5 });
  });

  it('unions the tail range into the mid-turn set while a turn runs', () => {
    const st = makeWindowingState();
    openFetches(st, 'c', 30, 'running');
    // the union widens from the empty {0,0} default, so the tail range {10,30}
    // becomes {0,30}.
    expect(st.duringTurn.get('c')).toEqual({ start: 0, end: 30 });
  });

  it('does not union while the turn is idle', () => {
    const st = makeWindowingState();
    openFetches(st, 'c', 30, 'idle');
    expect(st.duringTurn.get('c')).toBeUndefined();
  });
});

describe('turnEndFetches (turn-end re-read)', () => {
  it('re-reads the mid-turn union when the turn goes idle', () => {
    const st = makeWindowingState();
    st.duringTurn.set('c', { start: 0, end: 30 });
    expect(turnEndFetches(st, 'c', 'idle')).toEqual([{ session: 'c', start: 0, count: 30 }]);
    // the union is consumed by the re-read
    expect(st.duringTurn.get('c')).toBeUndefined();
  });

  it('is a no-op while the turn is not idle', () => {
    const st = makeWindowingState();
    st.duringTurn.set('c', { start: 0, end: 30 });
    expect(turnEndFetches(st, 'c', 'running')).toEqual([]);
    expect(st.duringTurn.get('c')).toEqual({ start: 0, end: 30 });
  });

  it('is a no-op when nothing was hydrated mid-turn', () => {
    const st = makeWindowingState();
    expect(turnEndFetches(st, 'c', 'idle')).toEqual([]);
  });
});

// The two policy constants the component used to own (now the module's).
describe('the policy constants', () => {
  it('keep the historical values', () => {
    expect(FETCH_MARGIN).toBe(5);
    expect(OPEN_TAIL).toBe(20);
  });
});
