// The transcript's paged read (spec §8): the GUI decides the window, the core
// serves the slice. The state lives in the bag (state.svelte).
import { command } from '../protocol';
import { resolveBlobs, upsertEntry } from '../entries';
import { store } from './state.svelte';

// Views keep the file's own id — the snapshot and this paged read must share
// one id namespace or their copies of an entry never match.
//
// In-flight dedupe: the transcript's window effect re-fires on every
// window recompute (the post-open height settling is a storm of them),
// and an identical (start, count) already in flight is shared, not
// re-issued — so the recompute storm cannot re-request the same page.
const windowsInFlight = new Map<string, Promise<void>>();

// In-flight blob reads keyed by workspace + hash: the window effect's
// recompute storm can hit the same blob twice; an identical read is
// shared, not re-issued (the windowsInFlight pattern).
const blobsInFlight = new Map<string, Promise<unknown>>();

export async function fetchWindow(sid: string, start: number, count: number): Promise<void> {
  const s = store.sessions[sid];
  if (!s || count <= 0) return;
  const key = `${sid}:${start}:${count}`;
  const inFlight = windowsInFlight.get(key);
  if (inFlight) return inFlight;
  const done = (async () => {
    try {
      const out = await command({
        type: 'session_entries',
        session: sid,
        since: null,
        range: { start, count }
      });
      // A re-open can replace the session with a fresh state while this read is in flight (and the
      // re-open's identical window request is deduped onto it), so
      // merging into the object captured at issue time would land the
      // page in the discarded state and leave the re-opened transcript's
      // window unhydrated.
      const cur = store.sessions[sid];
      if (!cur) return;
      if (out.kind !== 'entries') return;
      const views = await resolveBlobs(out.entries, (v) => {
        const key = `${cur.meta.workspace}:${v.blob!.hash}`;
        const inflight = blobsInFlight.get(key);
        if (inflight) return inflight;
        const read = command({
          type: 'blob_read',
          workspace: cur.meta.workspace,
          id: v.blob!.id,
          hash: v.blob!.hash
        })
          .then((o) => (o.kind === 'blob' ? o.payload : null))
          .finally(() => blobsInFlight.delete(key));
        blobsInFlight.set(key, read);
        return read;
      });
      for (const v of views) upsertEntry(cur.entries, v);
    } finally {
      windowsInFlight.delete(key);
    }
  })();
  windowsInFlight.set(key, done);
  return done;
}
