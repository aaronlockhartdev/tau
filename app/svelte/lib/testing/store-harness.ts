// Shared harness for the store suites: the workspace fixture,
// the snapshot/entry builders, and the default IPC route table. Each test
// file keeps its own hoisted mockInvoke and hands it to bindIPC().
import type { Mock } from 'vitest';
import type {
  Command,
  CommandOutput,
  LiveState,
  SessionMeta,
  SubagentInfo,
  Usage,
  ViewEntry,
  Workspace
} from '../wire/protocol';
import { store } from '../store/store.svelte';

export const WS: Workspace = { id: 'w1', name: 'proj', cwd: '/tmp/proj' };

export function meta(id: string, workspace = WS.id, extra: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id,
    workspace,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false,
    ...extra
  };
}

export function snap(sid: string, extra: { session?: Partial<SessionMeta>; live?: Partial<LiveState> } = {}): {
  kind: 'snapshot';
  snapshot: import('../wire/protocol').Snapshot;
} {
  return {
    kind: 'snapshot',
    snapshot: {
      workspace: WS,
      session: meta(sid, WS.id, extra.session),
      entries: [],
      om: { observation_tokens: 0, pending_tokens: 0, reflector_threshold: 0 },
      live: {
        queue: [],
        turn: 'idle',
        gate_failed: false,
        plan: { steps: [], criteria: '', notes: '' },
        subagents: [],
        own_state: null,
        ...extra.live
      },
      cursor: ''
    }
  };
}

export function entryView(id: string, kind: string, payload: Record<string, unknown>): ViewEntry {
  return {
    id,
    parent: null,
    kind,
    timestamp: 1,
    payload,
    blob: null,
    first_kept: null
  };
}

export const USAGE: Usage = { input_tokens: 5, output_tokens: 10, total_tokens: 15, cached_prompt_tokens: 0 };

// A sub-agent mirror row, complete (the wire shape the events carry).
export function sub(child: string, extra: Partial<SubagentInfo> = {}): SubagentInfo {
  return {
    handle: child,
    child,
    agent_type: 'general',
    context_mode: 'fresh',
    state: 'idle',
    waiting_on: null,
    last_message: null,
    usage: null,
    resume_contract: null,
    ...extra
  };
}

// Bind the IPC helpers to a test file's hoisted mockInvoke so existing
// mockIPC(...)/defaultIPC(...) call sites need no signature change.
export function bindIPC(mockInvoke: Mock) {
  const mockIPC = (handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) => {
    mockInvoke.mockImplementation(async (_name: unknown, args: { command: Command }) => handler(args.command));
  };

  // The default route table: the commands a boot/switch flow issues.
  const defaultIPC = (over: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {}) => {
    const base: Record<Command['type'], (cmd: Command) => CommandOutput> = {
      workspace_list: () => ({ kind: 'workspaces', workspaces: [WS] }),
      workspace_open: () => ({ kind: 'workspace', workspace: WS }),
      workspace_close: () => ({ kind: 'none' }),
      session_list: () => ({ kind: 'sessions', sessions: [meta('s1')] }),
      session_new: () => ({ kind: 'session', session: meta('s2') }),
      session_rename: () => ({ kind: 'none' }),
      session_set_model: () => ({ kind: 'none' }),
      session_open: () => ({ kind: 'snapshot', snapshot: snap('s1').snapshot }),
      session_close: () => ({ kind: 'none' }),
      session_delete: () => ({ kind: 'none' }),
      session_archive: (cmd) =>
        cmd.type === 'session_archive'
          ? { kind: 'session', session: meta(cmd.session, WS.id, { archived: true }) }
          : { kind: 'none' },
      session_restore: () => ({ kind: 'none' }),
      session_fork: () => ({ kind: 'none' }),
      session_branch: () => ({ kind: 'none' }),
      session_snapshot: () => ({ kind: 'none' }),
      session_entries: () => ({ kind: 'entries', entries: [] }),
      blob_read: () => ({ kind: 'blob', payload: null }),
      message_send: () => ({ kind: 'none' }),
      message_stop: () => ({ kind: 'none' }),
      queue_item_remove: () => ({ kind: 'none' }),
      subagent_types: () => ({ kind: 'agents', agents: [] }),
      subagent_list: () => ({ kind: 'subagents', subagents: [] }),
      subagent_state: () => ({ kind: 'subagent', subagent: sub('c1') }),
      subagent_activity: () => ({ kind: 'subagent_activity', activity: { observations: [], live: [], degraded: false } }),
      subagent_spawn: () => ({ kind: 'none' }),
      subagent_message: () => ({ kind: 'none' }),
      subagent_stop: () => ({ kind: 'none' }),
      skill_list: () => ({ kind: 'skills', skills: [] }),
      provider_list: () => ({ kind: 'providers', providers: [] }),
      provider_add: () => ({ kind: 'none' }),
      provider_set: () => ({ kind: 'none' }),
      provider_delete: () => ({ kind: 'none' }),
      file_read: () => ({ kind: 'file', file: { text: '', truncated: false } }),
      file_list: () => ({ kind: 'files', files: [] }),
      file_unlist: () => ({ kind: 'none' }),
      git_status: () => ({ kind: 'git_status', status: { branch: 'main', modified: [], untracked: [], deleted: [] } }),
      git_show: () => ({ kind: 'git_show', text: '' })
    };
    mockIPC((cmd) => (over[cmd.type] ?? base[cmd.type])(cmd));
  };

  return { mockIPC, defaultIPC };
}

export function freshStore() {
  store.workspaces = [];
  store.current = null;
  store.sessions = {};
  store.files = {};
  store.fileErrors = {};
  store.skills = {};
  store.pane = {};
  store.tabSelected = [];
  store.tabSelAnchor = null;
  store.loading = false;
  store.error = null;
  store.tailJump = 0;
  store.reasoningOpen = true;
  store.modelMenuOpen = false;
  store.entryOpen = new Map();
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
}
