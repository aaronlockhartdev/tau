// Shared display formatters (F3): the "how do I print this value" answers
// that were copy-pasted across components.

// Compact age: "just now" under a minute, then whole minutes / hours.
export function fmtAgo(ms: number): string {
  const m = (Date.now() - ms) / 60000;
  return m < 1 ? 'just now' : m < 60 ? `${Math.round(m)}m` : `${Math.round(m / 60)}h`;
}

// Compact count: 1234 → "1.2k" (token counts, thresholds).
export function fmtK(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
}
