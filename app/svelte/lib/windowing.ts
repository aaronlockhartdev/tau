// The transcript's paged-hydration policy (F1): a pure state → fetch-requests
// function owning the hysteresis, the open-tail, the mid-turn union, and the
// turn-end re-read. Transcript's effects are thin adapters over this — they
// read the virtualizer/store and issue the returned fetches, with no policy
// logic of their own. The windowing bookkeeping (lastFetched / duringTurn) is
// the policy's own state, held in a WindowingState the caller creates.

export interface Range {
  start: number;
  end: number;
}

export interface FetchRequest {
  session: string;
  start: number;
  count: number;
}

export type TurnState = 'idle' | 'starting' | 'running';

export interface WindowingState {
  lastFetched: Map<string, Range>;
  duringTurn: Map<string, Range>;
  // The session the open-tail page was issued for: the one-shot guard (#42).
  openedFor: string | null;
}

export function makeWindowingState(): WindowingState {
  return { lastFetched: new Map(), duringTurn: new Map(), openedFor: null };
}

// Hysteresis margin (≈ one 600px buffer of 120px cards): a scroll only
// re-fetches once the visible range has drifted this far past the last fetch.
export const FETCH_MARGIN = 5;
// A session opens pinned at its tail, so the first page is this many entries.
export const OPEN_TAIL = 20;

// Union a range into a session's mid-turn set (widening the span).
function union(st: WindowingState, session: string, range: Range): void {
  const d = st.duringTurn.get(session) ?? { start: 0, end: 0 };
  st.duringTurn.set(session, {
    start: Math.min(d.start, range.start),
    end: Math.max(d.end, range.end)
  });
}

// A scroll moved the visible range: fetch it only if it has drifted beyond the
// last fetch by ~a buffer (the hysteresis); record it as the last fetch; while
// a turn is in flight, union it into the mid-turn set (a mid-turn read can
// capture a streamed assistant before it finalizes).
export function scrollFetches(
  st: WindowingState,
  session: string,
  visible: Range,
  turn: TurnState
): FetchRequest[] {
  const last = st.lastFetched.get(session);
  if (last && visible.start >= last.start - FETCH_MARGIN && visible.end <= last.end + FETCH_MARGIN) {
    return [];
  }
  st.lastFetched.set(session, { start: visible.start, end: visible.end });
  if (turn !== 'idle') union(st, session, visible);
  return [{ session, start: visible.start, count: visible.end - visible.start }];
}

// A session opens pinned at its tail: the first page is the tail. One-shot
// per session (#42): a turn change re-fires the effect (it reads the turn
// for the mid-turn union) without re-issuing the page, but the union is
// still recorded so the turn-end re-read heals. The caller drops openedFor
// when no session is live, so a closed session's reopen re-issues.
export function openFetches(
  st: WindowingState,
  session: string,
  total: number,
  turn: TurnState
): FetchRequest[] {
  const count = Math.min(OPEN_TAIL, total);
  const range: Range = { start: total - count, end: total };
  if (turn !== 'idle') union(st, session, range);
  if (st.openedFor === session) return [];
  st.openedFor = session;
  st.lastFetched.set(session, range);
  return [{ session, start: range.start, count }];
}

// A turn just ended: re-read the ranges hydrated while it ran (bypassing the
// scroll hysteresis) so entries finalized since the in-flight read heal. No-op
// while the turn is not idle, or if nothing was hydrated mid-turn.
export function turnEndFetches(
  st: WindowingState,
  session: string,
  turn: TurnState
): FetchRequest[] {
  if (turn !== 'idle') return [];
  const r = st.duringTurn.get(session);
  if (!r) return [];
  st.duringTurn.delete(session);
  return [{ session, start: r.start, count: r.end - r.start }];
}
