// Pure transcript-virtualizer math: prefix-sum windowing.
//
// The old computeWin walked the whole entry list three times per scroll frame
// (start scan, end scan, scrollBefore), each step a heights-Map lookup. At
// 10k entries that is ~15k lookups/frame ≈ 44 ms of a 16 ms budget, so WebKit
// dropped wheel delta (the burst-scroll stall). A cumulative-height prefix
// array, rebuilt only when a height changes, turns every of those into a
// binary search (O(log n)) and an O(1) offset read.
//
// These are pure (no Svelte, no DOM) so they are unit-testable in isolation.

// Cumulative heights: prefix[0] = 0, prefix[k] = sum of h[0..k). Length is
// h.length + 1, so prefix[i] is the y-offset of entry i and prefix[n] is the
// track total.
export function buildPrefix(h: number[]): Float64Array {
  const p = new Float64Array(h.length + 1);
  for (let i = 0; i < h.length; i++) p[i + 1] = p[i] + h[i];
  return p;
}

// First index i in [0, n-1] with prefix[i+1] >= target, or n when the target
// is past the total. Monotone, so a binary search. This is the exact
// semantics of the old linear "first i where the running sum reaches target"
// scan (see computeWin in Transcript's history): it breaks on the first entry
// whose cumulative bottom crosses the target.
export function lowerIndex(prefix: Float64Array, n: number, target: number): number {
  let lo = 0;
  let hi = n - 1;
  let ans = n;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (prefix[mid + 1] >= target) {
      ans = mid;
      hi = mid - 1;
    } else {
      lo = mid + 1;
    }
  }
  return ans;
}

export interface Win {
  start: number;
  end: number;
  offset: number;
}

// The windowed slice for a viewport at `top` of height `view` plus a `buffer`
// margin. Binary-searches the prefix; no full-list walk. Replicates the old
// computeWin exactly:
//   start = first entry whose bottom crosses `top` (or n past the total)
//   end   = min(n, max(start, first entry whose bottom crosses top+view+buffer) + 1)
//   offset = the y-position of `start` (prefix[start]).
export function computeWin(
  prefix: Float64Array,
  n: number,
  top: number,
  view: number,
  buffer: number
): Win {
  const start = lowerIndex(prefix, n, top);
  const endTarget = top + view + buffer;
  const endPre = lowerIndex(prefix, n, endTarget);
  const end = Math.min(n, Math.max(start, endPre) + 1);
  return { start, end, offset: prefix[start] };
}
