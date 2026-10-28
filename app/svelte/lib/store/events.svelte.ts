// The event application (ADR-0006): the wire's batches folded into the state
// bag (state.svelte). The live-stream clock lives in stream.svelte.ts; the
// subagent fold in sessions.ts.
import { type Event } from '../protocol';
import { streamClock } from '../stream.svelte';
import { upsertEntry } from '../entries';
import { applySubagentEvent, laneOf } from '../sessions';
import { store } from './state.svelte';
import { refreshGitStatus, scheduleRefetch } from './files-pane.svelte';
import { fetchActiveFile } from './viewer.svelte';
import { syncWorkspaces } from './workspace.svelte';

export function applyEvents(evts: Event[]): void {
  for (const ev of evts) {
    // Session-less events (the system group, skill_list_changed) carry
    // no `session` key at all — the presence check narrows the union.
    const sid = 'session' in ev ? ev.session : null;
    if (!sid) {
      if (ev.type === 'system') {
        // Only an error kind writes: an unrelated system event (from any
        // client) must not wipe a live IPC error (S3).
        if (ev.kind.kind === 'error') store.error = ev.kind.message;
        if (ev.kind.kind === 'workspace_opened') void syncWorkspaces();
      }
      if (ev.type === 'skill_list_changed') {
        // Full-state replacement (ticket #31): the change guard keeps an
        // unchanged re-emit a no-op.
        if (JSON.stringify(store.skills[ev.workspace] ?? []) !== JSON.stringify(ev.skills)) {
          store.skills[ev.workspace] = ev.skills;
        }
      }
      if (ev.type === 'file_tree_changed') {
        scheduleRefetch(ev.workspace, ev.changed);
        void refreshGitStatus(ev.workspace);
        // Live file-content view: a change under the active file's dir
        // (the watcher reports the parent dir) re-fetches its content + base
        // so the viewer tracks the file as it is edited.
        const af = store.activeFile;
        if (af && af.workspace === ev.workspace) {
          const touches = ev.changed.some((d) => af.path === d || af.path.startsWith(d + '/'));
          if (touches) void fetchActiveFile(af.workspace, af.path);
        }
      }
      continue;
    }
    const s = store.sessions[sid];
    if (!s) continue;
    // One clock read per event: the TPS math (the first-delta baseline,
    // #224) and the last-modified touch (#282/#283) share it — a second
    // read would drift between them (and advance fake timers in tests).
    const now = Date.now();
    switch (ev.type) {
      case 'stream_start': {
        streamClock.clearStream(ev.session);
        s.tps = 0;
        s.turn = 'running';
        if (now > s.last_modified) s.last_modified = now;
        break;
      }
      case 'stream_end': {
        if (ev.usage) {
          s.usage = ev.usage;
          // #224: the endpoint's decode-window duration wins when it gives
          // one; otherwise the window runs from the first live output. No
          // window, no rate.
          s.tps = streamClock.rate(ev.session, ev.usage, ev.duration_secs);
        }
        streamClock.clearStream(ev.session);
        // No tail re-read: the finalized lines already landed as upserts.
        s.turn = 'idle';
        if (now > s.last_modified) s.last_modified = now;
        break;
      }
      case 'stream_delta': {
        // #303: only the new characters cross the wire; the view appends
        // them in seq order and refuses an out-of-order delta. The entry's
        // file line, which lands as an upsert, is the recovery.
        if (!streamClock.apply(s.entries, ev)) break;
        // #224: the stream's first live text opens the TPS clock; the live
        // gauge runs on the accumulated card, not the one delta.
        s.tps = streamClock.gauge(ev.session, s.entries[ev.id]);
        if (now > s.last_modified) s.last_modified = now;
        break;
      }
      case 'entry_upsert': {
        upsertEntry(s.entries, ev.entry);
        // A tool call is the model's first output: leave 'starting'.
        if (s.turn === 'starting' && ev.entry.kind === 'tool') s.turn = 'running';
        // The entry's file line is the record: it replaces whatever the
        // deltas accumulated, so the seq tracker for this id is done.
        streamClock.settle(ev.session, ev.entry.id);
        // #224: a stream that streamed opened its TPS clock on the first
        // delta; a call that streamed no text (a tool-only call) opens it
        // here, measured from the first thing the model produced, so
        // stream_end's exact TPS has a base. No live gauge to estimate on.
        if (
          ev.entry.kind === 'assistant' &&
          (s.turn === 'running' || s.turn === 'starting') &&
          streamClock.firstDeltaAt(ev.session) === undefined
        ) {
          streamClock.open(ev.session);
        }
        if (now > s.last_modified) s.last_modified = now;
        break;
      }
      case 'queue': {
        s.pending = ev.items.map((q) => ({
          text: q.text,
          lane: laneOf(q.lane),
          source: q.source ?? undefined
        }));
        if (now > s.last_modified) s.last_modified = now;
        break;
      }
      case 'session_event': {
        if (ev.kind.kind === 'branch_move') s.meta.leaf = ev.kind.leaf;
        break;
      }
      case 'subagent_event': {
        // Idempotent-cumulative (spec §8): each event carries the full
        // state of one handle; a lost batch self-heals on the next
        // snapshot.
        // The parent is modified by its subagent activity too (#283):
        // applySubagentEvent advances the child, this advances the parent.
        store.sessions = applySubagentEvent(store.sessions, ev.session, ev.kind, now);
        // applySubagentEvent returns a new map: the parent row is a new
        // object, so s (captured above) is detached — re-read it.
        const ps = store.sessions[ev.session]!;
        if (now > ps.last_modified) ps.last_modified = now;
        break;
      }
      case 'om_status': {
        // The status bar's om gauge: the activity kind is event-driven;
        // the gauge values ride the snapshots.
        s.om.kind = ev.kind;
        break;
      }
      case 'plan_changed': {
        // The plan pane's data (spec §5.3): full-state replacement — the
        // watcher's parse renders in place; idempotent by construction.
        s.plan = ev.plan;
        if (now > s.last_modified) s.last_modified = now;
        break;
      }
      case 'system': {
        // Only an error kind writes (S3): the next successful command
        // clears the banner, not an unrelated system event.
        if (ev.kind.kind === 'error') {
          store.error = ev.kind.message;
          // A turn that failed before its first output (a provider 4xx, a
          // no output event to move the turn out of 'starting'.
          if (s.turn === 'starting') s.turn = 'idle';
        }
        break;
      }
    }
  }
}
