import { describe, it, expect } from 'vitest';
import { buildPrefix, computeWin } from './virtualizer';

// Reference implementation: the ORIGINAL 3-pass linear scan that
// Transcript.svelte's computeWin used. The prefix-sum version must return
// byte-identical {start, end, offset} for every input.
function computeWinScan(h: number[], top: number, view: number, buffer: number) {
  const n = h.length;
  let acc = 0;
  let start = 0;
  for (let i = 0; i < n; i++) {
    const hh = h[i];
    if (acc + hh >= top) {
      start = i;
      break;
    }
    acc += hh;
    start = i + 1;
  }
  let end = start;
  acc = 0;
  for (let i = 0; i < n; i++) {
    acc += h[i];
    if (acc >= top + view + buffer) {
      end = Math.max(start, i);
      break;
    }
    end = i + 1;
  }
  let offset = 0;
  for (let j = 0; j < start; j++) offset += h[j];
  return { start, end: Math.min(n, end + 1), offset };
}

describe('virtualizer', () => {
  it('buildPrefix computes cumulative heights', () => {
    expect([...buildPrefix([1, 2, 3, 4])]).toEqual([0, 1, 3, 6, 10]);
    expect([...buildPrefix([])]).toEqual([0]);
  });

  it('matches the original linear scan across random height arrays + scroll positions', () => {
    let seed = 12345;
    const rand = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
    for (let trial = 0; trial < 300; trial++) {
      const n = 1 + Math.floor(rand() * 500);
      const h = Array.from({ length: n }, () => (rand() < 0.3 ? 0 : Math.floor(rand() * 200)));
      const prefix = buildPrefix(h);
      const total = prefix[n];
      for (let k = 0; k < 40; k++) {
        const top = rand() * (total + 50); // sometimes past the total
        const view = 400 + Math.floor(rand() * 400);
        const buffer = 600;
        expect(computeWin(prefix, n, top, view, buffer)).toEqual(
          computeWinScan(h, top, view, buffer)
        );
      }
    }
  });

  it('matches the scan on the 10k tool-card worst case', () => {
    const n = 10000;
    const h = new Array<number>(n).fill(120);
    const prefix = buildPrefix(h);
    for (const frac of [0, 0.25, 0.5, 0.75, 0.999, 1.0, 1.0001]) {
      const top = frac * n * 120;
      expect(computeWin(prefix, n, top, 800, 600)).toEqual(computeWinScan(h, top, 800, 600));
    }
  });

  it('BENCH: binary search is far cheaper than the 3-pass scan at 10k', () => {
    const n = 10000;
    const h = new Array<number>(n).fill(120);
    const prefix = buildPrefix(h);
    const ITERS = 3000;
    for (let k = 0; k < 200; k++) {
      computeWinScan(h, k * 300, 800, 600);
      computeWin(prefix, n, k * 300, 800, 600);
    }
    let t0 = performance.now();
    for (let k = 0; k < ITERS; k++) computeWinScan(h, (k / ITERS) * n * 120, 800, 600);
    const scanMs = performance.now() - t0;
    t0 = performance.now();
    for (let k = 0; k < ITERS; k++) computeWin(prefix, n, (k / ITERS) * n * 120, 800, 600);
    const binMs = performance.now() - t0;
    // eslint-disable-next-line no-console
    console.log(
      `  BENCH n=${n}: scan ${(scanMs / ITERS * 1000).toFixed(1)} µs/call → binary ` +
        `${(binMs / ITERS * 1000).toFixed(2)} µs/call → ${Math.round(scanMs / binMs)}× faster`
    );
    expect(binMs).toBeLessThan(scanMs);
  });
});
