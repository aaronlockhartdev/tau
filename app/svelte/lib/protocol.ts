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

// A provider as the GUI sees it (no key material, §8): the model menu's
// grouping source.
export interface ProviderInfo {
  name: string;
  base_url: string;
  models: string[];
}

// An agent type as the GUI sees it (the sub-agent spawn menu's data
// source; core lib.rs AgentType, the ADR-0006 field-for-field mirror).
export interface AgentInfo {
  name: string;
  description: string;
  builtin: boolean;
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
  // Archive is one-way and off the live path (ADR-0005): a true flag means
  // the file sits in the workspace archive dir, listed for the GUI only.
  archived: boolean;
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

// The entry payload shapes (C9): what each kind's writer appends — the
// ADR-0006 mirror of the crate's payload module. The file stays free-form
// at the ADR-0005 storage level; these types own the shapes the GUI decodes.
export type SkillRef = { name: string; location: string };

export interface UserPayload {
  text: string;
  lane: MessageLane;
  source?: string;
  skill?: SkillRef;
}

// One tool call an assistant message issued (the provider's shape, spec §6).
export interface FunctionCall {
  id: string;
  call_id: string;
  name: string;
  arguments: string;
}

export interface AssistantPayload {
  text: string;
  reasoning: string;
  interrupted: boolean;
  usage?: Usage;
  calls: FunctionCall[];
}

export interface ToolPayload {
  call_id: string;
  name: string;
  args: Record<string, unknown>;
  output: unknown;
}

// The OM record (crates/tau-core/src/om.rs OmRecord) as the `om` entry's
// payload: the newest entry wins, so the decoder shows what it added.
export interface OmPayload {
  frozen_prefix: string;
  active_observations: string;
  cursor?: { entry_id: string; timestamp: number };
  generation: number;
  observation_tokens: number;
  pending_tokens: number;
  prefix_demoted?: boolean;
}

export interface SystemPayload {
  note: string;
}

export interface SpawnSnapshotPayload {
  parentSession: string;
  range: string;
  log: string;
}

export type SubagentPayload =
  | { event: 'spawn'; type: string; brief: string; context_mode: string; parent: string; call: string; task?: string }
  | { event: 'state'; state: 'running' }
  | { event: 'state'; state: 'idle'; waiting_on: string }
  | { event: 'state'; state: 'done' }
  | { event: 'state'; state: 'failed'; reason: string }
  | { event: 'state'; state: 'stopped'; by: string }
  | { event: 'notify'; text: string; done: boolean; output?: Record<string, unknown>; waiting_on?: string };

// The per-session task event (crates/tau-core/src/task.rs): the task state
// is a fold of these; the card renders the event plus its variant fields.
export type TaskPayload =
  | { event: 'created'; id: string; title: string; steps: Step[]; criteria: Criterion[] }
  | { event: 'started'; id: string }
  | { event: 'evidence'; id: string; evidence: Evidence }
  | { event: 'finished'; id: string; force: boolean; reason: string | null }
  | { event: 'blocked'; id: string; reason: string; needs: string | null }
  | {
      event: 'assigned';
      id: string;
      // creator's copy: the worker's session; worker's copy: the record.
      worker?: string;
      record?: { title: string; status: string; steps: Step[]; criteria: Criterion[]; evidence: Evidence[]; blockers: Blocker[]; created_in: string };
    }
  | { event: 'handed_off'; id: string; output: Record<string, unknown> }
  | { event: 'cancelled'; id: string; reason: string }
  | { event: 'pointer'; id: string; status: string }
  | { event: 'note'; id: string; text: string };

// The flat card shape the transcript renders: a ViewEntry payload decoded
// (or the snapshot's metadata skeleton before a paged read fills it in).
// One variant per kind (C9); the last is the escape for a kind the card
// renders generically (a compaction record, a pre-v0 file).
export type Entry =
  | { id: string; kind: 'user'; text: string; source?: string; skill?: SkillRef; msg?: { prose: string; kv: Array<{ k: string; lines: string[] }> } }
  | MessageEntry
  | { id: string; kind: 'tool'; text?: string; name?: string; args?: Record<string, unknown>; output?: string; status?: 'ok' | 'running' | 'error' }
  | { id: string; kind: 'om'; text: string }
  | { id: string; kind: 'system'; text: string }
  | { id: string; kind: 'spawn-snapshot'; text: string }
  | { id: string; kind: 'subagent'; text: string; payload?: SubagentPayload }
  | { id: string; kind: 'task'; text: string; payload?: TaskPayload }
  | AnyEntry;

// A streamed assistant message in flight: the live slot until stream_end
// promotes it (an interrupted partial keeps its slot's id).
export type MessageEntry = {
  id: string;
  kind: 'message' | 'interrupted';
  text: string;
  reasoning?: string;
  calls?: string[];
  usage?: Usage;
};

// The escape variant: every variant's fields, all optional — kinds the
// card renders as a plain text block (no payload interpretation).
export type AnyEntry = {
  id: string;
  kind: string;
  text?: string;
  source?: string;
  skill?: SkillRef;
  reasoning?: string;
  calls?: string[];
  usage?: Usage;
  name?: string;
  args?: Record<string, unknown>;
  output?: string;
  status?: 'ok' | 'running' | 'error';
  payload?: SubagentPayload | TaskPayload;
  msg?: { prose: string; kv: Array<{ k: string; lines: string[] }> };
};
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
  current_step: Step | null;
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
  // The session's OM gauge (ticket #22): the observation tokens against the
  // session's configured reflector threshold, plus the unobserved pending.
  om: {
    observation_tokens: number;
    pending_tokens: number;
    reflector_threshold: number;
  };
  live: LiveState;
  cursor: string;
}

export interface EntryRange {
  start: number;
  count: number;
}

export interface FileEntry {
  name: string;
  path: string;
  dir: boolean;
  size: number;
}

export type Command =
  | { type: 'workspace_open'; cwd: string }
  | { type: 'workspace_list' }
  | { type: 'workspace_close'; workspace: string }
  | { type: 'session_list'; workspace: string }
  | { type: 'session_new'; workspace: string; title: string | null }
  | { type: 'session_rename'; session: string; title: string }
  | { type: 'session_set_model'; session: string; model: string }
  | { type: 'session_open'; session: string }
  | { type: 'session_close'; session: string }
  | { type: 'session_delete'; session: string }
  | { type: 'session_archive'; session: string }
  | { type: 'session_restore'; workspace: string; session: string }
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
  | { type: 'file_read'; workspace: string; path: string; offset: number | null; limit: number | null }
  | { type: 'file_list'; workspace: string; path: string };

export type CommandOutput =
  | { kind: 'none' }
  | { kind: 'workspace'; workspace: Workspace }
  | { kind: 'workspaces'; workspaces: Workspace[] }
  | { kind: 'session'; session: SessionMeta }
  | { kind: 'sessions'; sessions: SessionMeta[] }
  | { kind: 'snapshot'; snapshot: Snapshot }
  | { kind: 'entries'; entries: ViewEntry[] }
  | { kind: 'providers'; providers: ProviderInfo[] }
  | { kind: 'agents'; agents: AgentInfo[] }
  | { kind: 'skills'; skills: SkillInfo[] }
  | { kind: 'subagent'; subagent: SubagentInfo }
  | { kind: 'subagents'; subagents: SubagentInfo[] }
  | { kind: 'file'; file: { text: string; truncated: boolean } }
  | { kind: 'files'; files: FileEntry[] };

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
  // The session's OM activity (the turn-end Observer/Reflector run): the
  // status bar's om gauge — busy while in flight, idle when it finishes.
  | { type: 'om_status'; workspace: string; session: string; kind: 'observing' | 'reflecting' | 'idle' }
  | { type: 'system'; workspace: string; session: string | null; kind: SystemEventKind }
  | { type: 'subagent_event'; workspace: string; session: string; kind: SubagentEventKind }
  | { type: 'task_changed'; workspace: string; session: string; tasks: Task[] }
  // The workspace's skill registry changed (the file watcher, ticket #31):
  // full-state replacement, idempotent; session-less like the system group.
  | { type: 'skill_list_changed'; workspace: string; skills: SkillInfo[] }
  // The workspace's tree changed (the file watcher, ticket #32): the stale
  // dir paths (workspace-relative; `.` is the root) — the store refetches
  // the affected listed dirs. Session-less like the system group.
  | { type: 'file_tree_changed'; workspace: string; changed: string[] };

export type ProtocolError =
  | { kind: 'unsupported'; message: string }
  | { kind: 'not_found'; what: string }
  | { kind: 'other'; message: string };

// Transport #1 (ADR-0002: tau-core/tau-protocol are free of Tauri types; the
// GUI is the one place that sees them). The dev-only entry (no Tauri window)
// never calls these.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export async function command(cmd: Command): Promise<CommandOutput> {
  try {
    return (await invoke<CommandOutput>('tau_command', { command: cmd })) as CommandOutput;
  } catch (e) {
    // The Rust side rejects with the deserialized ProtocolError: a plain
    // object carrying message (other/unsupported) or what (not_found) —
    // neither stringifies to anything useful on its own.
    const o = e as { message?: string; what?: string } | string;
    const msg = typeof o === 'string' ? o : o?.message ?? o?.what ?? String(o);
    throw new Error(msg);
  }
}

export function onEvents(cb: (events: Event[]) => void): Promise<UnlistenFn> {
  return listen<Event[]>('tau://event', (e) => cb(e.payload));
}

// Tauri transport detection (ADR-0006): a plain browser page
// runs without the Tauri window.
export function isTauri(): boolean {
  return '__TAURI_INTERNALS__' in window;
}
