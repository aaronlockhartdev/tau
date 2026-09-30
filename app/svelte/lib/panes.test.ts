import { describe, expect, it } from 'vitest';
import { applyTabSelect, defaultPane, paneOf } from './panes';

const plain = { shiftKey: false, metaKey: false, ctrlKey: false };
const cmd = { shiftKey: false, metaKey: true, ctrlKey: false };
const shift = { shiftKey: true, metaKey: false, ctrlKey: false };

describe('defaultPane', () => {
  it('defaults to the sessions tab, unexpanded, no selection', () => {
    const p = defaultPane();
    expect(p.ltab).toBe('sessions');
    expect(p.rtab).toBe('tasks');
    expect(p.historyOpen).toBe(false);
    expect(p.openGroups).toBeNull();
    expect(p.selected).toEqual([]);
    expect(p.selAnchor).toBeNull();
  });
});

describe('paneOf', () => {
  it('is null for a null workspace', () => {
    expect(paneOf({ w1: defaultPane() }, null)).toBeNull();
  });

  it('is null for an uncreated workspace', () => {
    expect(paneOf({ w1: defaultPane() }, 'w2')).toBeNull();
  });

  it('returns the pane for a created workspace', () => {
    const p = defaultPane();
    expect(paneOf({ w1: p }, 'w1')).toBe(p);
  });
});

describe('applyTabSelect', () => {
  it('cmd/ctrl toggles a tab in and out', () => {
    let t = applyTabSelect({ selected: [], anchor: null }, ['w1', 'w2'], 'w1', cmd);
    expect(t).toEqual({ selected: ['w1'], anchor: 'w1' });
    t = applyTabSelect(t, ['w1', 'w2'], 'w1', cmd);
    expect(t).toEqual({ selected: [], anchor: 'w1' });
  });

  it('ctrl is equivalent to cmd', () => {
    const t = applyTabSelect({ selected: [], anchor: null }, ['w1'], 'w1', { shiftKey: false, metaKey: false, ctrlKey: true });
    expect(t.selected).toEqual(['w1']);
  });

  it('shift spans from the anchor in tab order, both directions', () => {
    const base = { selected: ['w1'], anchor: 'w1' };
    expect(applyTabSelect(base, ['w1', 'w2', 'w3'], 'w3', shift).selected).toEqual(['w1', 'w2', 'w3']);
    expect(applyTabSelect(base, ['w1', 'w2', 'w3'], 'w2', shift).selected).toEqual(['w1', 'w2']);
    const from = { selected: ['w3'], anchor: 'w3' };
    expect(applyTabSelect(from, ['w1', 'w2', 'w3'], 'w1', shift).selected).toEqual(['w1', 'w2', 'w3']);
  });

  it('shift without a known anchor falls back to the first selected, then to the clicked tab', () => {
    expect(applyTabSelect({ selected: ['w2'], anchor: null }, ['w1', 'w2', 'w3'], 'w3', shift).selected).toEqual(['w2', 'w3']);
    expect(applyTabSelect({ selected: [], anchor: null }, ['w1', 'w2'], 'w2', shift).selected).toEqual(['w2']);
  });

  it('shift with an anchor outside the tab order is a no-op', () => {
    const base = { selected: ['w9'], anchor: 'w9' };
    expect(applyTabSelect(base, ['w1', 'w2'], 'w1', shift)).toBe(base);
  });

  it('a plain click clears the selection and sets the anchor', () => {
    const t = applyTabSelect({ selected: ['w1', 'w2'], anchor: 'w1' }, ['w1', 'w2'], 'w2', plain);
    expect(t).toEqual({ selected: [], anchor: 'w2' });
  });
});
