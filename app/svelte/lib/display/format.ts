// Shared display formatters (F3): the "how do I print this value" answers
// that were copy-pasted across components.
import prettyMilliseconds from 'pretty-ms';

// Compact age: "just now" under a minute, then the single largest unit
// (m / h / d / y) via pretty-ms's compact mode.
export function fmtAgo(ms: number, now: number = Date.now()): string {
  const age = now - ms;
  return age < 60_000 ? 'just now' : prettyMilliseconds(age, { compact: true });
}

// Compact count: 1234 → "1.2k" (token counts, thresholds).
export function fmtK(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
}

// The compact units pretty-ms emits for an age of a minute or more, as
// milliseconds. `bucketAge` reads that output back, so this table is the
// formatter's vocabulary: change the compact spelling and this table changes
// with it. The coupling is the point — one label, one bucket.
const COMPACT_UNIT: Record<string, number> = {
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
  y: 31_536_000_000
};

// The age bucket a timestamp falls in, keyed by the age `fmtAgo` *displays*: the
// compact unit floored, so two timestamps that render the same label compare
// equal and a stable sort keeps their prior order. A list sorted on this stops
// reordering on a tick that changes no label; crossing a bucket boundary is the
// only thing that moves a row (#322). Sub-minute ages stay raw — `just now` rows
// keep full precision, so a session with a live commit still leads its group. A
// future timestamp floors to 0 and sorts newest.
export function bucketAge(ts: number, now: number = Date.now()): number {
  const age = now - ts;
  if (age < 0) return 0;
  if (age < 60_000) return age;
  const label = prettyMilliseconds(age, { compact: true });
  const floor = Number(label.slice(0, -1)) * (COMPACT_UNIT[label.at(-1) ?? ''] ?? 1);
  return Math.min(floor, age);
}
