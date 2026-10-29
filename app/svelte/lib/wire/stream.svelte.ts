import { applyDelta } from './entries';
import type { Entry, Usage } from './protocol';

// The live stream's clock: which delta of an entry the view has applied, and
// when the stream's first visible character arrived (the #224 TPS base). Both
// are keyed by session before entry id, because entry ids are minted per
// session and repeat across sessions. None of this is the record: the entry's
// file line is, and it recovers any frame the clock refuses to apply.
const firstDeltaMs = new Map<string, number>();
const deltaSeq = new Map<string, Map<string, number>>();

function openClock(session: string): number {
  let first = firstDeltaMs.get(session);
  if (first === undefined) {
    first = Date.now();
    firstDeltaMs.set(session, first);
  }
  return first;
}

// The window the rate runs on: #224 starts it at the first live output, which
// excludes prefill (a stream-start clock would wrongly include it). A window
// under 0.2 s is noise, so it yields no rate.
function windowSecs(baseMs: number): number {
  return (Date.now() - baseMs) / 1000;
}

export const streamClock = {
  // A call starts and the last call ends: its frames and its TPS base are
  // done, and a stale seq must not veto a re-minted entry id.
  clearStream(session: string): void {
    firstDeltaMs.delete(session);
    deltaSeq.delete(session);
  },

  // Append a delta to its card; false when it is not the next frame for that
  // entry, which leaves the card at what the view has already seen.
  apply(
    entries: Record<string, Entry>,
    ev: { session: string; id: string; seq: number; text: string; reasoning: string }
  ): boolean {
    const seqs = deltaSeq.get(ev.session) ?? new Map<string, number>();
    deltaSeq.set(ev.session, seqs);
    return applyDelta(entries, seqs, ev);
  },

  // The entry's file line landed: it replaces whatever the deltas
  // accumulated, so this entry's frame run is over.
  settle(session: string, id: string): void {
    deltaSeq.get(session)?.delete(id);
  },

  // Open the clock on the session's first live output — the first streamed
  // text, or for a call that streamed none, the first entry it produced — and
  // hand back its base.
  open(session: string): number {
    return openClock(session);
  },

  firstDeltaAt(session: string): number | undefined {
    return firstDeltaMs.get(session);
  },

  // The live gauge: a token-rate estimate from the card's accumulated text,
  // not from one delta. AnyEntry's escape variant also matches kind
  // 'message', so text is read as the wire data it is.
  gauge(session: string, card: Entry | undefined): number {
    const secs = windowSecs(openClock(session));
    if (secs <= 0.2) return 0;
    const text = card && card.kind === 'message' ? (card.text ?? '') : '';
    return Math.max(1, Math.round(text.length / 4)) / secs;
  },

  // The exact rate a call ends on: the endpoint's decode-window duration
  // (#224) wins when it provides one, otherwise the clock's window.
  rate(session: string, usage: Usage, durationSecs: number | null | undefined): number {
    const first = firstDeltaMs.get(session);
    const secs = durationSecs ?? (first !== undefined ? windowSecs(first) : null);
    return secs !== null && secs > 0.2 ? usage.output_tokens / secs : 0;
  }
};
