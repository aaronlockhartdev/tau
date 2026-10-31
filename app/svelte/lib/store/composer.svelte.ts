// The composer's round-trips (ticket #25): send, stop, and the queued
// message's removal. The state lives in the bag (state.svelte).
import { command } from '../wire/protocol';
import { errText } from '../wire/errors';
import { type PendingMsg } from '../rules/sessions';
import { clearError, sessionOf, store } from './state.svelte';

export async function send(text: string, lane: PendingMsg['lane']): Promise<void> {
  const sid = store.current;
  if (sid === null || text.trim() === '') return;
  const s = sessionOf(sid);
  s.pending = s.pending.filter((p) => !(p.text === text && p.lane === lane));
  // No optimistic card (ADR-0008): the core mints the entry id and the
  // card lands on the send's entry_upsert echo (~ms away on the local
  // channel); a rejected send simply never echoes.
  store.tailJump++;
  try {
    clearError();
    await command({
      type: 'message_send',
      session: sid,
      text,
      lane: lane === 'follow-up' ? 'follow_up' : lane
    });
    // The turn is dispatched: it is now 'starting' — waiting for the model's
    // first output (prefill + request latency). stream_start / a tool upsert
    // it to running; the waiting indicator and the stop button key off this.
    if (s.turn === 'idle') s.turn = 'starting';
  } catch (e) {
    store.error = errText(e);
  }
}

export async function stop(): Promise<void> {
  const sid = store.current;
  if (sid === null) return;
  try {
    clearError();
    await command({ type: 'message_stop', session: sid });
  } catch (e) {
    store.error = errText(e);
  }
}

// Delete a queued item by the id the core minted for it (#346): the row
// drops optimistically, and the core's queue event is the resync authority.
export async function deleteQueueItem(id: string): Promise<void> {
  const sid = store.current;
  if (sid === null) return;
  const s = sessionOf(sid);
  s.pending = s.pending.filter((p) => p.id !== id);
  try {
    clearError();
    await command({ type: 'queue_item_remove', session: sid, item: id });
  } catch {
    // optimistic: a queue event will resync (a concurrent turn boundary
    // may have consumed the item, in which case the resync is the truth)
  }
}
