// Rejections can be plain objects (a serialized core error) — String()
// of one is "[object Object]".

/** A core refusal the caller branches on instead of showing a banner: the
 * `busy` kind is a running-turn refusal, retryable once the turn ends (#346). */
export class BusyError extends Error {
  constructor(what: string) {
    super(what);
    this.name = 'BusyError';
  }
}

export function errText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object') {
    const o = e as { message?: unknown; label?: unknown; what?: unknown };
    if (typeof o.message === 'string') return o.message;
    if (typeof o.label === 'string') return o.label;
    if (typeof o.what === 'string') return o.what;
    try {
      return JSON.stringify(e);
    } catch {
      const last: unknown = e;
      return String(last);
    }
  }
  return String(e);
}
