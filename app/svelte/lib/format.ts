// Shared display formatters (F3): the "how do I print this value" answers
// that were copy-pasted across components.
import prettyMilliseconds from 'pretty-ms';

// Compact age: "just now" under a minute, then the single largest unit
// (m / h / d / y) via pretty-ms's compact mode.
export function fmtAgo(ms: number): string {
  const age = Date.now() - ms;
  return age < 60_000 ? 'just now' : prettyMilliseconds(age, { compact: true });
}

// Compact count: 1234 → "1.2k" (token counts, thresholds).
export function fmtK(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
}
