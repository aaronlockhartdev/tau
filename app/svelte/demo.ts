// The dev-only demo entry (ticket #29): the product's own boot path
// against an emulated core. mockIPC (the @tauri-apps/api test double)
// answers tau_command from the 10k-entry fixture, so workspace boot,
// session open, and the paged reads run unmodified store code with zero
// network. scripts/verify-demo.mjs drives this page; the release build
// excludes it (vite ships it only in 'demo' mode).
import './app.css';
import { mount } from 'svelte';
import { mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import App from './App.svelte';
import { store, init, applyEvents } from './lib/store.svelte';
import {
  buildDemoSession,
  demoChildren,
  demoSkills,
  demoSubagents,
  demoTasks
} from './lib/fixture';
import type {
  Command,
  CommandOutput,
  FileEntry,
  EntryMeta,
  Event,
  Snapshot,
  Usage,
  Workspace
} from './lib/protocol';

const WS: Workspace = { id: 'w-demo', name: 'tau', cwd: '~/git/tau' };
// The files-pane fixture listing (ticket #32): the demo answers file_list
// from this table, and the rig mutates it to prove the invalidation wave
// refetches content, not just re-renders.
export const demoFiles: Record<string, FileEntry[]> = {
  '.': [
    { name: 'src', path: 'src', dir: true, size: 0 },
    { name: 'README.md', path: 'README.md', dir: false, size: 12 }
  ],
  src: [{ name: 'main.rs', path: 'src/main.rs', dir: false, size: 42 }]
};
const { meta, entries, views } = buildDemoSession();
const kids = demoChildren();
// The bar's usage segment: the fixture parent's usage is the sum of its
// children's, so it shows real tokens (the raw fixture meta carries none).
const PARENT_USAGE: Usage = { input_tokens: 195800, output_tokens: 62200, total_tokens: 258000, cached_prompt_tokens: 0 };

// The snapshot's entry metadata: the 80-char preview skeletons the store
// hydrates in place from the paged reads.
const entryMetas: EntryMeta[] = views.map((v, i) => ({
  id: v.id,
  parent: v.parent,
  kind: v.kind,
  timestamp: v.timestamp,
  size: JSON.stringify(v.payload ?? '').length,
  preview: entries[i].text ?? '',
  first_kept: v.first_kept,
  status: 'ok'
}));

function snapshotFor(sid: string): Snapshot {
  if (sid === meta.id) {
    return {
      workspace: WS,
      session: { ...meta, usage: PARENT_USAGE },
      entries: entryMetas,
      om: null,
      live: {
        queue: [
          { text: 'use the 62-char alphabet, not base36', lane: 'steering' },
          { text: 'before you finish, run cargo fmt', lane: 'steering' },
          { text: 'also update CONTEXT.md with the new terms', lane: 'follow_up' }
        ],
        turn: 'idle',
        subagents: demoSubagents(),
        tasks: demoTasks()
      },
      cursor: meta.leaf ?? ''
    };
  }
  const c = kids.find((k) => k.meta.id === sid);
  if (!c) throw new Error(`demo entry: unknown session ${sid}`);
  return {
    workspace: WS,
    session: { ...c.meta, usage: c.usage },
    entries: [],
    om: null,
    live: {
      queue: [],
      turn: c.state === 'running' ? 'running' : 'idle',
      subagents: c.subagents,
      tasks: c.tasks
    },
    cursor: ''
  };
}

function mockBackend(cmd: string, args?: unknown): CommandOutput | string {
  // The File → Open Folder… picker (ticket #29 B1): the demo entry mocks the
  // dialog plugin's command with a fixed folder; the store's openWorkspace
  // runs unmodified against it.
  if (cmd === 'plugin:dialog|open') return '/tmp/tau-rig-ws';
  const c = (args as { command?: Command } | undefined)?.command;
  if (cmd !== 'tau_command' || !c) throw new Error(`demo entry: unmocked IPC ${cmd}`);
  switch (c.type) {
    case 'workspace_list':
      return { kind: 'workspaces', workspaces: [WS] };
    case 'workspace_open': {
      // The core keys workspaces by cwd; echo a stable id for non-demo
      // folders (the File → Open Folder… rig check).
      if (c.cwd !== WS.cwd) {
        const name = c.cwd.split(/[\\/]/).filter(Boolean).pop() ?? c.cwd;
        return { kind: 'workspace', workspace: { id: 'w-rig', name, cwd: c.cwd } };
      }
      return { kind: 'workspace', workspace: WS };
    }
    case 'skill_list':
      return { kind: 'skills', skills: demoSkills() };
    // The fixture session first: the boot rule opens the list's head.
    case 'session_list':
      return { kind: 'sessions', sessions: [meta, ...kids.map((k) => k.meta)] };
    case 'session_archive':
      // The one-way move (ADR-0005): the demo just flags the row.
      return {
        kind: 'session',
        session: { ...(store.sessions[c.session]?.meta ?? meta), archived: true }
      };
    case 'session_open': {
      const snap = snapshotFor(c.session);
      // switchSession keeps only parent/state/archived from the previous
      // row, so an already-applied waiting_on annotation (B3) would die
      // with the rebuild; the wire has no field for it, so the demo entry
      // restores the row's own pre-rebuild value.
      const w = store.sessions[c.session]?.waiting_on;
      if (w !== null && w !== undefined) {
        setTimeout(() => {
          const row = store.sessions[c.session];
          if (row) row.waiting_on = w;
        }, 0);
      }
      return { kind: 'snapshot', snapshot: snap };
    }
    case 'session_entries':
      if (c.session === meta.id && c.range) {
        return { kind: 'entries', entries: views.slice(c.range.start, c.range.start + c.range.count) };
      }
      return { kind: 'entries', entries: [] };
    case 'message_send':
    case 'message_stop':
      return { kind: 'none' };
    case 'file_list':
      return { kind: 'files', files: demoFiles[c.path] ?? [] };
    default:
      throw new Error(`demo entry: unmocked command ${c.type}`);
  }
}

// The wire's SessionMeta carries no per-session state, so boot's stubs
// need the fixture's child states (the pane badges), MRU order, and
// archive flags seeded on top.
function seedKids(): void {
  for (const c of kids) {
    const s = store.sessions[c.meta.id];
    if (!s) continue;
    s.state = c.state;
    s.turn = c.turn;
    s.usage = c.usage;
    s.mru = c.mru;
    s.archived = c.archived;
    s.waiting_on = c.waiting_on;
    s.subagents = c.subagents;
    s.tasks = c.tasks;
  }
}

// Two live child streams: generated at 47 ms, flushed (coalesced) at
// 25 ms — the prototype's exact cadence, through the real consumer.
function startStreams(): void {
  const ws = WS.id;
  const sid = meta.id;
  const lines = [
    '  let buf = &mut self.buf;',
    '  while let Some((i, line)) = scan_line(buf) {',
    '      match self.state {',
    '          St::Sse => self.on_line(line)?,',
    '          St::Json => self.on_chunk(line)?,',
    '      }',
    '  }',
    '  Ok(())'
  ];
  const streams = [
    { call: 'demo-live-1', tok: 0, buf: '', sent: 0, started: false },
    { call: 'demo-live-2', tok: 3, buf: '', sent: 0, started: false }
  ];
  const dirty = new Set<string>();
  setInterval(() => {
    for (const s of streams) {
      s.buf += (s.buf ? '\n' : '') + lines[s.tok % lines.length];
      s.tok++;
      dirty.add(s.call);
    }
  }, 47);
  setInterval(() => {
    if (dirty.size === 0) return;
    const batch: Event[] = [];
    for (const c of dirty) {
      const s = streams.find((x) => x.call === c)!;
      if (!s.started) {
        s.started = true;
        batch.push({ type: 'stream_start', workspace: ws, session: sid, call_id: s.call });
      }
      // Deltas are incremental (appended since the last flush, spec §8);
      // the consumer accumulates.
      const inc = s.buf.slice(s.sent);
      s.sent = s.buf.length;
      if (inc) {
        batch.push({
          type: 'stream_delta',
          workspace: ws,
          session: sid,
          call_id: s.call,
          text: inc,
          reasoning: null
        });
      }
    }
    dirty.clear();
    applyEvents(batch);
  }, 25);
}

mockIPC(mockBackend, { shouldMockEvents: true });
init().then(() => {
  seedKids();
  startStreams();
});
// The rig drives File → Open Folder…'s store-side path (ticket #29 B1):
// the native menu emits, mockIPC's event mock carries it to the listener
// the store registered in init().
const seam = (window as unknown as {
  __tau?: { openFolderRequest?: () => void; demoFiles?: Record<string, FileEntry[]> };
}).__tau;
if (seam) {
  seam.openFolderRequest = () => void emit('open_folder_requested');
  seam.demoFiles = demoFiles;
}

const root = document.getElementById('app');
if (!root) throw new Error('missing #app element');
mount(App, { target: root });
