import { describe, expect, it } from 'vitest';
import { closeIn, openIn, type OpenFile } from './openfiles';

const f = (path: string): OpenFile => ({ path, ignored: false, size: 1 });

describe('openIn', () => {
  it('adds a new file', () => {
    const { list, added } = openIn([f('a.ts')], f('b.ts'));
    expect(added).toBe(true);
    expect(list.map((x) => x.path)).toEqual(['a.ts', 'b.ts']);
  });

  it('is a no-op for an already-open file', () => {
    const { list, added } = openIn([f('a.ts')], f('a.ts'));
    expect(added).toBe(false);
    expect(list.map((x) => x.path)).toEqual(['a.ts']);
  });
});

describe('closeIn', () => {
  it('keeps the tab that shifts into the closed slot', () => {
    const { list, activate } = closeIn([f('a'), f('b'), f('c')], 'b');
    expect(list.map((x) => x.path)).toEqual(['a', 'c']);
    expect(activate).toBe(1);
  });

  it('activates the next tab when closing the first', () => {
    const { activate } = closeIn([f('a'), f('b')], 'a');
    expect(activate).toBe(0);
  });

  it('activates the left neighbour when closing the last', () => {
    const { list, activate } = closeIn([f('a'), f('b'), f('c')], 'c');
    expect(list.map((x) => x.path)).toEqual(['a', 'b']);
    expect(activate).toBe(1);
  });

  it('returns null when the last file closes', () => {
    const { list, activate } = closeIn([f('a')], 'a');
    expect(list).toHaveLength(0);
    expect(activate).toBeNull();
  });

  it('is a no-op for an unknown path', () => {
    const { list, activate } = closeIn([f('a')], 'zzz');
    expect(list.map((x) => x.path)).toEqual(['a']);
    expect(activate).toBeNull();
  });
});
