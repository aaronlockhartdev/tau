// The protocol's JSON shapes (crates/tau-protocol) as the Svelte side
// compiles against (ADR-0006: the GUI compiles against the protocol types
// alone). Tauri is transport #1: one command carries any Command; events
// arrive coalesced on `tau://event`.

export type MessageLane = 'force' | 'steering' | 'follow_up';

export type ContextMode = 'fresh' | 'compacted' | 'fork';

export interface Workspace {
  id: string;
  name: string;
  cwd: string;
}

// A discovered skill (the composer autocomplete's data source):
// location is the absolute SKILL.md path; model_invocation: false marks
// a catalog-excluded skill — the dropdown is its only door.
export interface SkillInfo {
  name: string;
  description: string;
  location: string;
  model_invocation: boolean;
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cached_prompt_tokens: number;
}

export interface SessionMeta {
  id: string;
  workspace: string;
  title: string | null;
  parent: string | null;
  created: number;
  leaf: string | null;
  model: string | null;
  usage: Usage | null;
}

export type EntryStatus = 'ok' | 'interrupted';

// The snapshot's entry metadata: everything the entry tree needs, no payload.
export interface EntryMeta {
  id: string;
  parent: string | null;
  kind: string;
  timestamp: number;
  size: number;
  preview: string;
  first_kept: string | null;
  status?: EntryStatus;
}

// A sidecar-blob pointer (ADR-0005): the payload lives out-of-band.
export interface BlobRef {
  id: string;
  size: number;
  hash: string;
}

export interface ViewEntry {
  id: string;
  parent: string | null;
  kind: string;
  timestamp: number;
  payload: unknown;
  blob: BlobRef | null;
  first_kept: string | null;
}

// The flat card shape the transcript renders: a ViewEntry payload unfolded
// (or the snapshot's metadata skeleton before a paged read fills it in).
export interface Entry {
  id: string;
  kind: string;
  text?: string;
  // The child session that produced this wake message (a sub-agent
  // notification is a user entry carrying its source, ticket #23).
  source?: string;
  // A /skill: invocation (ticket #28): the user entry records the
  // expanded template and carries the skill's identity for the block.
  skill?: { name: string; location: string };
  reasoning?: string;
  name?: string;
  args?: string;
  status?: 'ok' | 'running' | 'error';
  output?: string;
  usage?: Usage;
}
export interface QueuedItem {
  text: string;
  lane: MessageLane;
}

export type TurnState = 'idle' | 'running';

// Task shapes (ticket #24; crates/tau-core/src/task.rs) — tasks are
// per-session entries; the snapshot's LiveState carries the fold.
export type TaskStatus = 'pending' | 'in_progress' | 'done' | 'blocked' | 'cancelled';
export type StepStatus = 'pending' | 'active' | 'done' | 'skipped';
export type CriterionStatus = 'pending' | 'satisfied' | 'failed' | 'skipped';

export interface Step {
  text: string;
  expected_output: string;
  status: StepStatus;
}

export interface Criterion {
  text: string;
  status: CriterionStatus;
}

export interface Evidence {
  criterion: string;
  summary: string;
  command?: string;
  artifact?: string;
  passed: boolean;
  step?: string;
}

export interface Blocker {
  reason: string;
  needs?: string;
}

export interface Decision {
  question: string;
  decision: string;
  decided_by: string;
  rationale?: string;
}

export interface WorkerPointer {
  session: string;
  status: string;
}

export interface ResumeContract {
  task: string;
  title: string;
  status: string;
  current_step: { text: string; expected_output: string } | null;
  steps: Step[];
  evidence: Evidence[];
  gaps: string[];
  blockers: Blocker[];
  next_action: string;
}

export interface Task {
  id: string;
  title: string;
  status: TaskStatus;
  steps: Step[];
  criteria: Criterion[];
  evidence: Evidence[];
  blockers: Blocker[];
  decisions: Decision[];
  notes: string[];
  worker?: WorkerPointer;
  created_in?: string;
  updated: number;
  resume_contract?: ResumeContract;
}

// Sub-agent shapes (ticket #23; crates/tau-protocol) — a child is an
// ordinary session; this is its structured mirror in the parent's state.
export interface SubagentInfo {
  handle: string;
  child: string;
  agent_type: string;
  context_mode: ContextMode;
  state: 'running' | 'idle' | 'done' | 'failed' | 'stopped';
  waiting_on: string | null;
  last_message: string | null;
  usage: Usage | null;
  task: Task | null;
  resume_contract: ResumeContract | null;
}

export type SubagentEventKind =
  | { kind: 'spawned'; handle: string; child: string; agent_type: string; context_mode: ContextMode; title: string }
  | { kind: 'state'; handle: string; child: string; state: string; detail: unknown; note: string | null }
  | { kind: 'notified'; child: string; wake: string; text: string; output: unknown };

export interface LiveState {
  queue: QueuedItem[];
  turn: TurnState;
  subagents: SubagentInfo[];
  // Tasks are event-driven (task_changed) and snapshot-fed.
  tasks: Task[];
}

export interface Snapshot {
  workspace: Workspace;
  session: SessionMeta;
  entries: EntryMeta[];
  om: unknown;
  live: LiveState;
  cursor: string;
}

export interface EntryRange {
  start: number;
  count: number;
}

export type Command =
  | { type: 'workspace_open'; cwd: string }
  | { type: 'workspace_list' }
  | { type: 'session_list'; workspace: string }
  | { type: 'session_new'; workspace: string; title: string | null }
  | { type: 'session_rename'; session: string; title: string }
  | { type: 'session_open'; session: string }
  | { type: 'session_close'; session: string }
  | { type: 'session_delete'; session: string }
  | { type: 'session_fork'; session: string; at: string }
  | { type: 'session_branch'; session: string; at: string }
  | { type: 'session_snapshot'; session: string }
  | { type: 'session_entries'; session: string; since: string | null; range: EntryRange | null }
  | { type: 'message_send'; session: string; text: string; lane: MessageLane }
  | { type: 'message_stop'; session: string }
  | { type: 'subagent_types' }
  | { type: 'subagent_list'; session: string }
  | { type: 'subagent_state'; handle: string }
  | { type: 'subagent_spawn'; session: string; agent_type: string; brief: string; context_mode: ContextMode }
  | { type: 'subagent_message'; handle: string; text: string | null }
  | { type: 'subagent_stop'; handle: string }
  | { type: 'task_create'; session: string; title: string }
  | { type: 'task_update'; session: string; task: string; note: string }
  | { type: 'task_assign'; session: string; task: string; worker: string }
  | { type: 'task_evidence'; session: string; task: string; criterion: string; summary: string; passed: boolean | null }
  | { type: 'task_cancel'; session: string; task: string }
  | { type: 'skill_list'; workspace: string }
  | { type: 'provider_list' }
  | { type: 'provider_add'; name: string; base_url: string; key_env: string; models: string[] }
  | { type: 'provider_set'; name: string; base_url: string; key_env: string; models: string[] }
  | { type: 'provider_delete'; name: string }
  | { type: 'file_read'; workspace: string; path: string; offset: number | null; limit: number | null };

export type CommandOutput =
  | { kind: 'none' }
  | { kind: 'workspace'; workspace: Workspace }
  | { kind: 'workspaces'; workspaces: Workspace[] }
  | { kind: 'session'; session: SessionMeta }
  | { kind: 'sessions'; sessions: SessionMeta[] }
  | { kind: 'snapshot'; snapshot: Snapshot }
  | { kind: 'entries'; entries: ViewEntry[] }
  | { kind: 'providers'; providers: Array<{ name: string; base_url: string; models: string[] }> }
  | { kind: 'agents'; agents: Array<{ name: string; description: string; builtin: boolean }> }
  | { kind: 'skills'; skills: SkillInfo[] }
  | { kind: 'subagent'; subagent: SubagentInfo }
  | { kind: 'subagents'; subagents: SubagentInfo[] }
  | { kind: 'file'; file: { text: string; truncated: boolean } };

export type SessionEventKind =
  | { kind: 'branch_move'; leaf: string }
  | { kind: 'compaction'; entry: string; first_kept: string };

export type SystemEventKind =
  | { kind: 'workspace_opened'; name: string; cwd: string }
  | { kind: 'provider_changed' }
  | { kind: 'error'; message: string };

export type Event =
  | { type: 'stream_start'; workspace: string; session: string; call_id: string }
  | { type: 'stream_delta'; workspace: string; session: string; call_id: string; text: string; reasoning: string | null }
  | { type: 'stream_end'; workspace: string; session: string; call_id: string; interrupted: boolean; usage: Usage | null }
  | { type: 'tool_start'; workspace: string; session: string; call_id: string; tool_call_id: string; name: string }
  | { type: 'tool_end'; workspace: string; session: string; call_id: string; tool_call_id: string; name: string; output: unknown }
  | { type: 'queue'; workspace: string; session: string; items: QueuedItem[] }
  | { type: 'session_event'; workspace: string; session: string; kind: SessionEventKind }
  | { type: 'system'; workspace: string; session: string | null; kind: SystemEventKind }
  | { type: 'subagent_event'; workspace: string; session: string; kind: SubagentEventKind }
  | { type: 'task_changed'; workspace: string; session: string; tasks: Task[] }
  // The workspace's skill registry changed (the file watcher, ticket #31):
  // full-state replacement, idempotent; session-less like the system group.
  | { type: 'skill_list_changed'; workspace: string; skills: SkillInfo[] };

export type ProtocolError =
  | { kind: 'unsupported'; message: string }
  | { kind: 'not_found'; what: string }
  | { kind: 'other'; message: string };

// Transport #1 (ADR-0002: tau-core/tau-protocol are free of Tauri types; the
// GUI is the one place that sees them). The demo path (no Tauri window)
// never calls these.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export async function command(cmd: Command): Promise<CommandOutput> {
  try {
    return (await invoke<CommandOutput>('tau_command', { command: cmd })) as CommandOutput;
  } catch (e) {
    // The Rust side returns ProtocolError as a serialized string payload.
    const msg = typeof e === 'string' ? e : (e as { message?: string })?.message ?? String(e);
    throw new Error(msg);
  }
}

export function onEvents(cb: (events: Event[]) => void): Promise<UnlistenFn> {
  return listen<Event[]>('tau://event', (e) => cb(e.payload));
}

// Tauri transport detection (ADR-0006): the browser demo path (?demo=1)
// runs without the Tauri window.
export function isTauri(): boolean {
  return '__TAURI_INTERNALS__' in window;
}
