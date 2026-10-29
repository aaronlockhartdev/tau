// The shared multiselect rule (F4): single click, multi toggle, and shift
// range — the positive paths and the stale-anchor / out-of-order negatives.

import { describe, expect, it } from 'vitest';
import { nextSelection } from './selection';

const ORD = ['a', 'b', 'c', 'd'];
const plain = { shift: false, multi: false };
const multi = { shift: false, multi: true };
const shift = { shift: true, multi: false };

describe('nextSelection', () => {
  it('a plain click clears the selection', () => {
    expect(nextSelection(ORD, ['a', 'b'], null, 'c', plain)).toEqual([]);
    expect(nextSelection(ORD, [], 'a', 'd', plain)).toEqual([]);
  });

  it('multi toggles the clicked id in and out', () => {
    expect(nextSelection(ORD, [], 'a', 'a', multi)).toEqual(['a']);
    expect(nextSelection(ORD, ['a'], 'a', 'b', multi)).toEqual(['a', 'b']);
    expect(nextSelection(ORD, ['a', 'b'], 'a', 'a', multi)).toEqual(['b']);
  });

  it('shift spans from the anchor, forward and backward', () => {
    expect(nextSelection(ORD, ['a'], 'a', 'c', shift)).toEqual(['a', 'b', 'c']);
    expect(nextSelection(ORD, ['c'], 'c', 'a', shift)).toEqual(['a', 'b', 'c']);
    expect(nextSelection(ORD, ['a', 'b'], 'a', 'b', shift)).toEqual(['a', 'b']);
  });

  it('shift falls back to the first selected, then to the clicked id', () => {
    expect(nextSelection(ORD, ['b'], null, 'd', shift)).toEqual(['b', 'c', 'd']);
    expect(nextSelection(ORD, [], null, 'c', shift)).toEqual(['c']);
  });

  it('a click outside the current selection is just a plain or multi click', () => {
    expect(nextSelection(ORD, ['b'], null, 'd', plain)).toEqual([]);
    expect(nextSelection(ORD, ['b'], 'b', 'd', multi)).toEqual(['b', 'd']);
  });

  it('an empty selection shift-spans from the clicked id', () => {
    expect(nextSelection(ORD, [], null, 'b', shift)).toEqual(['b']);
  });

  it('a single-item list toggles and spans', () => {
    expect(nextSelection(['a'], [], null, 'a', multi)).toEqual(['a']);
    expect(nextSelection(['a'], ['a'], 'a', 'a', multi)).toEqual([]);
    expect(nextSelection(['a'], ['a'], 'a', 'a', shift)).toEqual(['a']);
  });

  it('shift with a stale anchor is a no-op for the keep fallback', () => {
    expect(nextSelection(ORD, ['x'], 'x', 'a', { ...shift, shiftFallback: 'keep' })).toBeNull();
    expect(nextSelection(ORD, ['a'], 'a', 'x', { ...shift, shiftFallback: 'keep' })).toBeNull();
  });

  it('shift with a stale anchor selects the clicked id for the clicked fallback', () => {
    expect(
      nextSelection(ORD, ['x'], 'x', 'a', { ...shift, shiftFallback: 'clicked' })
    ).toEqual(['a']);
    expect(
      nextSelection(ORD, ['a'], 'a', 'x', { ...shift, shiftFallback: 'clicked' })
    ).toEqual(['x']);
  });
});
