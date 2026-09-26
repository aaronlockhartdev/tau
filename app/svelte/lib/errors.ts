// Rejections can be plain objects (a serialized core error) — String()
// of one is "[object Object]".
export function errText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object') {
    const o = e as { message?: unknown; label?: unknown };
    if (typeof o.message === 'string') return o.message;
    if (typeof o.label === 'string') return o.label;
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }
  return String(e);
}
