import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fmtAgo, fmtK, bucketAge } from './format';

const HOUR = 3_600_000;
const DAY = 86_400_000;

describe('fmtAgo', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-01-01T12:00:00Z'));
  });
  afterEach(() => vi.useRealTimers());

  const now = new Date('2026-01-01T12:00:00Z').getTime();
  const ago = (ms: number) => now - ms;

  it('is "just now" under a minute', () => {
    expect(fmtAgo(ago(0))).toBe('just now');
    expect(fmtAgo(ago(59_999))).toBe('just now');
  });

  it('uses the single largest unit, scaling up past hours', () => {
    expect(fmtAgo(ago(90_000))).toBe('1m'); // 90s floors to a minute
    expect(fmtAgo(ago(2 * HOUR))).toBe('2h');
    expect(fmtAgo(ago(14 * HOUR))).toBe('14h'); // the case that motivated the change
    expect(fmtAgo(ago(30 * HOUR))).toBe('1d'); // was "30h"
    expect(fmtAgo(ago(14 * DAY))).toBe('14d'); // was "336h"
    expect(fmtAgo(ago(400 * DAY))).toBe('1y'); // 400d is past a year
  });

  it('honours an explicit now, so a shared clock tick advances the label', () => {
    expect(fmtAgo(now - 14 * HOUR, now)).toBe('14h');
    expect(fmtAgo(now - 14 * HOUR, now + 24 * HOUR)).toBe('1d');
  });
});

describe('fmtK', () => {
  it('is plain under 1000, compact at and above', () => {
    expect(fmtK(999)).toBe('999');
    expect(fmtK(1234)).toBe('1.2k');
  });
});

// The sort key the session lists use (#322): the bucket is the label `fmtAgo`
// prints, so the order a user sees is the order the list keeps.
describe('bucketAge', () => {
  const now = new Date('2026-01-01T12:00:00Z').getTime();
  const ago = (ms: number) => now - ms;

  it('floors to the displayed unit at each boundary', () => {
    expect(bucketAge(ago(61_000), now)).toBe(60_000); // "1m"
    expect(bucketAge(ago(119_999), now)).toBe(60_000); // "1m", one second short of "2m"
    expect(bucketAge(ago(30 * HOUR), now)).toBe(DAY); // "1d", not "30h"
    expect(bucketAge(ago(400 * DAY), now)).toBe(31_536_000_000); // "1y"
  });

  it('is the floor of the label, never past the age itself', () => {
    expect(bucketAge(ago(60_000), now)).toBe(60_000);
    expect(bucketAge(ago(2 * HOUR + 1), now)).toBe(2 * HOUR);
  });

  it('puts two timestamps that read the same in one bucket', () => {
    // The churn this fixes: 2h01m and 2h14m both read "2h", and raw sorting
    // swapped them on every write to last_modified.
    expect(bucketAge(ago(2 * HOUR + 60_000), now)).toBe(bucketAge(ago(2 * HOUR + 14 * 60_000), now));
    expect(fmtAgo(ago(2 * HOUR + 60_000), now)).toBe(fmtAgo(ago(2 * HOUR + 14 * 60_000), now));
  });

  it('keeps the list steady across a write that changes no label', () => {
    // An age, so ascending is newest first — the pane's order. Two rows that read
    // the same label compare equal, and sort's stability keeps them as listed:
    // that is the churn this removes, a row jumping under the cursor.
    const byAge = (ts: number[]) => [...ts].sort((x, y) => bucketAge(x, now) - bucketAge(y, now));
    const listed = ago(2 * HOUR + 10 * 60_000);
    const written = ago(2 * HOUR + 1 * 60_000);
    expect(fmtAgo(written, now)).toBe(fmtAgo(listed, now));
    expect(byAge([listed, written])).toEqual([listed, written]);
    // The same pair on raw stamps puts the write first — the reorder users saw.
    expect([listed, written].sort((x, y) => y - x)).toEqual([written, listed]);
  });

  it('moves a row when it crosses a label', () => {
    const byAge = (at: number, ts: number[]) =>
      [...ts].sort((x, y) => bucketAge(x, at) - bucketAge(y, at));
    // 2h59m reads "2h" and sits beside the other "2h" row; two minutes of clock
    // later it reads "3h" and drops below it. Crossing is the only reorder.
    const crossing = ago(2 * HOUR + 59 * 60_000);
    const beside = ago(2 * HOUR + 30 * 60_000);
    expect(fmtAgo(crossing, now)).toBe('2h');
    expect(byAge(now, [crossing, beside])).toEqual([crossing, beside]);
    expect(fmtAgo(crossing, now + 2 * 60_000)).toBe('3h');
    expect(byAge(now + 2 * 60_000, [crossing, beside])).toEqual([beside, crossing]);
  });

  it('keeps a sub-minute age at full precision', () => {
    // "just now" is one label for the whole minute, and these are the live rows:
    // flooring them to one bucket would drop a session with a fresh commit below
    // a session it just overtaken.
    expect(bucketAge(ago(5_000), now)).toBe(5_000);
    expect(bucketAge(ago(59_999), now)).toBe(59_999);
    expect(bucketAge(ago(1_000), now)).toBeLessThan(bucketAge(ago(30_000), now));
  });

  it('floors a future timestamp to 0, so it sorts newest', () => {
    expect(bucketAge(now + 60 * 60_000, now)).toBe(0);
    expect(bucketAge(now + 1, now)).toBe(0);
  });
});
