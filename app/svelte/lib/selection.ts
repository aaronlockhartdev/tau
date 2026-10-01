// The multi-select rule shared by the tab strip (applyTabSelect) and the
// session tree (SessionNode): cmd/ctrl toggles the clicked id in and out,
// shift spans from the anchor in the given order, a plain click empties the
// selection.

export interface SelectMods {
  shift: boolean;
  multi: boolean;
  // A shift click whose anchor (or the clicked id) is missing from
  // orderedIds: 'keep' means no-op, 'clicked' means select just the clicked
  // id.
  shiftFallback?: 'keep' | 'clicked';
}

// null = the selection is unchanged (a stale shift anchor with the 'keep'
// fallback).
export function nextSelection(
  orderedIds: string[],
  current: string[],
  anchor: string | null,
  clickedId: string,
  mods: SelectMods
): string[] | null {
  if (mods.shift) {
    const from = anchor ?? current[0] ?? clickedId;
    const a = orderedIds.indexOf(from);
    const b = orderedIds.indexOf(clickedId);
    if (a < 0 || b < 0) return mods.shiftFallback === 'clicked' ? [clickedId] : null;
    const [lo, hi] = a < b ? [a, b] : [b, a];
    return orderedIds.slice(lo, hi + 1);
  }
  if (mods.multi) {
    return current.includes(clickedId)
      ? current.filter((x) => x !== clickedId)
      : [...current, clickedId];
  }
  return [];
}
