import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fmtAgo, fmtK } from './format';

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
