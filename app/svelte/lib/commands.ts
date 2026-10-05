// The session-command cluster (F5, out of store.svelte, ticket #42): the
// clear-banner → invoke → refetch → reapply dance, spelled once. The store
// re-exports these so the components' imports are unchanged. Imports the
// store (a module cycle) — safe because every use is lazy, inside a function
// the store itself calls.
import { command, type Command, type CommandOutput, type SessionMeta } from './protocol';
import { applySessionList, openSession, setArchived } from './sessions';
import { errText } from './errors';
import { pane, store } from './store.svelte';

// Session-command convergence (F5): the clear-banner → invoke → on-failure
// banner core, spelled once. The per-command follow-up (reapply, and for the
// list-converging commands the session_list refetch) is passed in; onFail is
// a per-command failure hook (switch's rollback). Returns invoke success.
export async function runSessionCommand(
  cmd: Command,
  followUp: (out: CommandOutput) => void | Promise<void>,
  onFail?: () => void
): Promise<boolean> {
  try {
    store.error = null;
    const out = await command(cmd);
    await followUp(out);
    return true;
  } catch (e) {
    store.error = errText(e);
    onFail?.();
    return false;
  }
}

// The list-convergence refetch: re-read the workspace's session list and
// hand it to the per-command apply action. A failed refetch surfaces the
// banner (the primary command already succeeded).
export async function refetchSessionList(
  ws: string,
  apply: (list: SessionMeta[]) => void
): Promise<void> {
  try {
    const out = await command({ type: 'session_list', workspace: ws });
    if (out.kind === 'sessions') apply(out.sessions);
  } catch (e) {
    store.error = errText(e);
  }
}

// New top-level session in the workspace: the core names it (adjective-noun)
// when no title is given. Opens it and drops straight into inline rename so
// the generated name becomes a real one if the user cares. ⌘N (App.svelte)
// and the sessions tab's ghost row both call this.
export async function newSession(ws: string, title: string | null = null): Promise<string | null> {
  let sid: string | null = null;
  const ok = await runSessionCommand(
    { type: 'session_new', workspace: ws, title, base_prompt: null },
    (out) => {
      if (out.kind !== 'session') throw new Error('unexpected session_new output');
      sid = out.session.id;
    }
  );
  if (!ok || sid === null) return null;
  await switchSession(sid);
  const q = pane(ws);
  if (q) q.renamingId = sid;
  return sid;
}

export async function renameSession(sid: string, title: string): Promise<void> {
  const t = title.trim();
  if (!t) return;
  await runSessionCommand({ type: 'session_rename', session: sid, title: t }, () => {
    const s = store.sessions[sid];
    if (s) s.meta.title = t;
  });
}

// Archive (ADR-0005): one-way, off the live read/write path. The core
// stops a running child first and archives the session's children with
// it; the row keeps its state (a message resumes it) and moves to the
// archive folder, the children's rows converge on the refetched list.
export async function archiveSession(sid: string): Promise<void> {
  // A child is archived by its parent's cascade, never directly — a
  // standalone child archive would leave it out of the parent's row in
  // the archive folder.
  if (store.sessions[sid]?.meta.parent) return;
  const ok = await runSessionCommand(
    { type: 'session_archive', session: sid },
    (out) => {
      if (out.kind === 'session') store.sessions = setArchived(store.sessions, sid, out.session);
    }
  );
  if (!ok) return;
  const wsid = store.sessions[sid]?.meta.workspace;
  if (wsid)
    await refetchSessionList(wsid, (list) => {
      store.sessions = applySessionList(store.sessions, list);
    });
}

// Restore (ADR-0005): the file moves back from the archive dir (the core
// restores a parent's children with it); the rows converge on the
// refetched list, the archive flag's authority.
export async function restoreSession(ws: string, sid: string): Promise<void> {
  const ok = await runSessionCommand(
    { type: 'session_restore', workspace: ws, session: sid },
    () => {}
  );
  if (!ok) return;
  await refetchSessionList(ws, (list) => {
    store.sessions = applySessionList(store.sessions, list);
  });
}

// Permanent delete (the inverse of archive): the file is removed from disk
// (live or archive dir) and the row leaves the list. The command returns no
// id list, so the cascade's victims are found by diffing the refetch.
export async function deleteSession(ws: string, sid: string): Promise<void> {
  const ok = await runSessionCommand({ type: 'session_delete', session: sid }, () => {});
  if (!ok) return;
  // The refetch no longer carries the deleted id (nor its child): drop the
  // workspace's sessions that fell out of the alive set, then converge.
  await refetchSessionList(ws, (list) => {
    const alive = new Set(list.map((s) => s.id));
    const next = { ...store.sessions };
    for (const id of Object.keys(next)) {
      if (next[id]!.meta.workspace === ws && !alive.has(id)) delete next[id];
    }
    store.sessions = applySessionList(next, list);
  });
}

export async function switchSession(sid: string): Promise<void> {
  const prev = store.current;
  store.current = sid;
  // A failed open must not leave current dangling at an unhydrated stub:
  // roll back to the previous session (the onFail hook).
  await runSessionCommand(
    { type: 'session_open', session: sid },
    (out) => {
      if (out.kind !== 'snapshot') throw new Error('unexpected session_open output');
      store.sessions = openSession(store.sessions, sid, out.snapshot);
    },
    () => {
      store.current = prev;
    }
  );
}
