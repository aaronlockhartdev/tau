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

// EntryKind in Rust: the kinds v0 defines plus `Other`, which carries any other
// kind verbatim — the `(string & {})` arm keeps completion for the known ones
// while still admitting a kind an older client has never heard of.
export type EntryKind =
  | 'user'
  | 'assistant'
  | 'tool'
  | 'system'
  | 'om'
  | 'subagent'
  | 'spawn-snapshot'
  | 'diagnostic'
  | (string & {});

// The snapshot's entry metadata: everything the entry tree needs, no payload.
export interface EntryMeta {
  id: string;
  parent: string | null;
  kind: EntryKind;
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
  kind: EntryKind;
  timestamp: number;
  payload: unknown;
  blob: BlobRef | null;
  first_kept: string | null;
}

// The entry payload shapes (C9): what each kind's writer appends — the
// ADR-0006 mirror of the crate's payload module. The file stays free-form
// at the ADR-0005 storage level; these types own the shapes the GUI decodes.
export type SkillRef = { name: string; location: string };

export type SubagentPayload =
  | { event: 'spawn'; type: string; brief: string; context_mode: string; parent: string; call: string }
  | { event: 'state'; state: 'running' }
  | { event: 'state'; state: 'idle'; waiting_on: string }
  | { event: 'state'; state: 'done' }
  | { event: 'state'; state: 'failed'; reason: string }
  | { event: 'state'; state: 'stopped'; by: string }
  | { event: 'notify'; text: string; done: boolean; output?: Record<string, unknown>; waiting_on?: string };

// How a provider stream ended (ADR-0034 record vocabulary). Distinct from the
// 'stream_end' event, which says a turn settled after the OM pass.
export type StreamOutcome =
  | 'completed'
  | 'length'
  | 'idle_timeout'
  | 'eof_without_done'
  | 'mid_stream_error'
  | 'killed';

const streamOutcomes: StreamOutcome[] = [
  'completed',
  'length',
  'idle_timeout',
  'eof_without_done',
  'mid_stream_error',
  'killed'
];

// A wire value read as a stream ending (#307); anything else is not one.
export function asStreamOutcome(value: unknown): StreamOutcome | undefined {
  return streamOutcomes.find((o) => o === value);
}

// What an ending reads like in prose: one vocabulary for the card's badge and
// the diagnostic feed, so the same cut is spelled the same place to place (#307).
export const streamOutcomeLabels: Record<StreamOutcome, string> = {
  completed: 'finished cleanly',
  length: 'output cap',
  idle_timeout: 'stream went quiet',
  eof_without_done: 'stream ended early',
  mid_stream_error: 'provider error mid-stream',
  killed: 'stopped'
};

// The cut nobody asked for, or null when the ending is neither: a cut is an
// ending neither clean nor willed. The core decides the same way in `cutoff()`
// (#307, plan §6.11).
export function providerCutoff(value: unknown): StreamOutcome | null {
  const ending = asStreamOutcome(value);
  if (ending === undefined || ending === 'completed' || ending === 'killed') {
    return null;
  }
  return ending;
}

// The diagnostic codes this build knows (ADR-0034). Codes are append-only:
// a code from a newer core arrives through the escape arm and still renders,
// by its own name.
export type DiagnosticCode =
  | 'model_changed'
  | 'gate_unavailable'
  | 'context_overflow'
  | 'gate_attempt'
  | 'gate_failed'
  | 'om_stalled'
  | 'provider_rejected'
  | 'turn_cutoff';

// A fact the harness recorded about a session: the code is the discriminant
// and the GUI composes the sentence, so no display string crosses the seam.
export type Diagnostic =
  | { code: 'model_changed'; from?: string; to: string }
  | { code: 'gate_unavailable'; model: string; missing: string[] }
  | { code: 'context_overflow'; prompt: number; budget: number }
  | { code: 'gate_attempt'; command: string; retries: number; max: number; output: string }
  | { code: 'gate_failed'; command: string; retries: number }
  | { code: 'om_stalled'; pending: number; activation: number }
  | { code: 'provider_rejected'; status: number; body: string }
  | { code: 'turn_cutoff'; model: string; ended_by: StreamOutcome; blocks: number }
  // What a core reads back when a stored code is not one it knows: the record
  // on disk keeps its own code, this is the re-emitted form (Diagnostic::Unknown).
  | { code: 'unknown'; unknown_code: string }
  | { code: string };

// The flat card shape the transcript renders: a ViewEntry payload decoded
// (or the snapshot's metadata skeleton before a paged read fills it in).
// One variant per kind (C9); the last is the escape for a kind the card
// renders generically (a compaction record, a pre-v0 file).
export type Entry =
  | { id: string; kind: 'user'; text: string; source?: string; skill?: SkillRef; lane?: MessageLane; msg?: { prose: string; kv: Array<{ k: string; lines: string[] }> } }
  | MessageEntry
  | { id: string; kind: 'tool'; text?: string; name?: string; call_id?: string; args?: Record<string, unknown>; output?: string; ui_detail?: string; status?: 'ok' | 'running' | 'error' | 'cancelled'; unavailable?: boolean }
  | { id: string; kind: 'om'; text: string; thinking?: string; input?: string; suggestedResponse?: string; model?: string }
  | { id: string; kind: 'system'; text: string }
  | { id: string; kind: 'spawn-snapshot'; text: string }
  | { id: string; kind: 'subagent'; text: string; payload?: SubagentPayload }
  | { id: string; kind: 'diagnostic'; text: string; diagnostic: Diagnostic }
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
  // How the provider's response ended, as the provider told it; absent on a
  // record from before the field existed.
  endedBy?: StreamOutcome;
};

// The escape variant: every variant's fields, all optional — kinds the
// card renders as a plain text block (no payload interpretation).
export type AnyEntry = {
  id: string;
  kind: string;
  text?: string;
  source?: string;
  lane?: MessageLane;
  skill?: SkillRef;
  reasoning?: string;
  calls?: string[];
  usage?: Usage;
  name?: string;
  call_id?: string;
  args?: Record<string, unknown>;
  output?: string;
  status?: 'ok' | 'running' | 'error' | 'cancelled';
  payload?: SubagentPayload;
  msg?: { prose: string; kv: Array<{ k: string; lines: string[] }> };
  thinking?: string;
  input?: string;
  suggestedResponse?: string;
  model?: string;
};
export interface QueuedItem {
  text: string;
  lane: MessageLane;
  // Child provenance (a sub-agent's report): null for the user's own messages.
  source?: string | null;
}

export type TurnState = 'idle' | 'running';

export interface ResumeContract {
  // The plan's first open step (spec §5.3) — the child's resume contract.
  next_action: string;
}

// A child session's own lifecycle state (ticket #221): the `subagent` state
// entry's projection — the badge state plus its declared wait target.
export interface OwnState {
  state: SubagentState;
  waiting_on: string | null;
}
// The child's 6-state lifecycle (spec §5.2): the badge state, mirrored from
// the core's `ChildState::kind`. A shared alias: `SubagentInfo.state` (the
// parent's view) and `LiveState.own_state.state` (the child's own tab) are
// the same value.
export type SubagentState =
  | 'running'
  | 'idle'
  | 'awaiting'
  | 'done'
  | 'failed'
  | 'stopped';

// Sub-agent shapes (ticket #23; crates/tau-protocol) — a child is an
// ordinary session; this is its structured mirror in the parent's state.
export interface SubagentInfo {
  handle: string;
  child: string;
  agent_type: string;
  context_mode: ContextMode;
  state: SubagentState;
  waiting_on: string | null;
  last_message: string | null;
  /** The child session's created timestamp (epoch ms): the core's spawn-order sort key. */
  created?: number;
  usage: Usage | null;
  resume_contract: ResumeContract | null;
}

// A child's activity window (#203): its observation-log tail plus the
// un-observed raw tail, projected compact and tail-capped by the core.
export interface ChildActivity {
  observations: ActivityGroup[];
  live: ActivityEntry[];
  degraded: boolean;
}

export interface ActivityGroup {
  id: string;
  range: string;
  kind?: string;
  content: string;
}

export interface ActivityEntry {
  role: string;
  tool?: string;
  status?: string;
  text: string;
}
export type SubagentEventKind =
  | { kind: 'spawned'; handle: string; child: string; agent_type: string; context_mode: ContextMode; title: string }
  | { kind: 'state'; handle: string; child: string; state: string; detail: unknown; note: string | null }
  | { kind: 'notified'; child: string; wake: string; text: string; output: unknown };

// One plan step (a checkbox line of the plan file, spec §5.3): the plan
// pane's row.
export interface PlanStep {
  text: string;
  done: boolean;
}

// The parsed state of the session's plan file (spec §5.3): the plan pane's
// data. An empty plan (no steps) is a session with no plan file yet.
export interface Plan {
  steps: PlanStep[];
  criteria: string;
  notes: string;
}

export interface LiveState {
  queue: QueuedItem[];
  turn: TurnState;
  // Completion gate failed after max_retries (T2) — the status-bar badge.
  gate_failed: boolean;
  // The session's parsed plan file (spec §5.3) — the plan pane's data.
  plan: Plan;
  subagents: SubagentInfo[];
  // A child session's own lifecycle state (ticket #221): the badge state
  // plus its declared wait; null for a root session or a child with no
  // recorded state. The GUI badges a child's tab by it, not by turn.
  own_state: OwnState | null;
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
  /** In .gitignore: shown in the tree, greyed (styling only). */
  ignored: boolean;
  /** Git change state; null outside a git workspace. */
  git: FileGit | null;
}

export type FileGitState = 'modified' | 'untracked' | 'deleted';

export interface FileGit {
  state: FileGitState;
  /** numstat counts; null when unmeasurable (binary). */
  added: number | null;
  deleted: number | null;
}


export interface GitStatus {
  branch: string;
  modified: string[];
  untracked: string[];
  deleted: string[];
}

export type Command =
  | { type: 'workspace_open'; cwd: string }
  | { type: 'workspace_list' }
  | { type: 'workspace_close'; workspace: string }
  | { type: 'session_list'; workspace: string }
  | { type: 'session_new'; workspace: string; title: string | null; base_prompt: string | null }
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
  | { type: 'blob_read'; workspace: string; id: string; hash: string }
  | { type: 'message_send'; session: string; text: string; lane: MessageLane }
  | { type: 'message_stop'; session: string }
  | { type: 'queue_item_remove'; session: string; index: number }
  | { type: 'subagent_types' }
  | { type: 'subagent_list'; session: string }
  | { type: 'subagent_state'; handle: string }
  | { type: 'subagent_spawn'; session: string; agent_type: string; brief: string; context_mode: ContextMode }
  | { type: 'subagent_message'; handle: string; text: string | null }
  | { type: 'subagent_stop'; handle: string }
  | { type: 'subagent_activity'; handle: string }
  | { type: 'skill_list'; workspace: string }
  | { type: 'provider_list' }
  | { type: 'provider_add'; name: string; base_url: string; key_env: string; models: string[] }
  | { type: 'provider_set'; name: string; base_url: string; key_env: string; models: string[] }
  | { type: 'provider_delete'; name: string }
  | { type: 'file_read'; workspace: string; path: string; offset: number | null; limit: number | null }
  | { type: 'file_list'; workspace: string; path: string }
  | { type: 'file_unlist'; workspace: string; path: string }
  | { type: 'git_status'; workspace: string }
  | { type: 'git_show'; workspace: string; path: string };

export type CommandOutput =
  | { kind: 'none' }
  | { kind: 'workspace'; workspace: Workspace }
  | { kind: 'workspaces'; workspaces: Workspace[] }
  | { kind: 'session'; session: SessionMeta }
  | { kind: 'sessions'; sessions: SessionMeta[] }
  | { kind: 'snapshot'; snapshot: Snapshot }
  | { kind: 'entries'; entries: ViewEntry[] }
  | { kind: 'blob'; payload: unknown }
  | { kind: 'providers'; providers: ProviderInfo[] }
  | { kind: 'agents'; agents: AgentInfo[] }
  | { kind: 'skills'; skills: SkillInfo[] }
  | { kind: 'subagent'; subagent: SubagentInfo }
  | { kind: 'subagent_activity'; activity: ChildActivity }
  | { kind: 'subagents'; subagents: SubagentInfo[] }
  | { kind: 'file'; file: { text: string; truncated: boolean } }
  | { kind: 'files'; files: FileEntry[] }
  | { kind: 'git_status'; status: GitStatus }
  | { kind: 'git_show'; text: string };

export type SessionEventKind =
  | { kind: 'branch_move'; leaf: string }
  | { kind: 'compaction'; entry: string; first_kept: string };

export type SystemEventKind =
  | { kind: 'workspace_opened'; name: string; cwd: string }
  | { kind: 'provider_changed' }
  | { kind: 'error'; message: string };

export type Event =
  | { type: 'stream_start'; workspace: string; session: string; call_id: string }
  | { type: 'stream_end'; workspace: string; session: string; call_id: string; interrupted: boolean; usage: Usage | null; duration_secs?: number }
  // A transcript entry is upserted: the exact file-line view the session log
  // holds (or will hold when finalized). The store sets map[id] = entry — the
  // only live transcript op (ADR-0008). Carries the streaming assistant entry
  // (re-emitted frame-aligned as it grows), tool entries (one id, call→result),
  // and every discrete card. The card renders on first sight, updates in place.
  | { type: 'entry_upsert'; workspace: string; session: string; entry: ViewEntry }
  | { type: 'queue'; workspace: string; session: string; items: QueuedItem[] }
  | { type: 'session_event'; workspace: string; session: string; kind: SessionEventKind }
  // The session's OM activity (the turn-end Observer/Reflector run): the
  // status bar's om gauge — busy while in flight, idle when it finishes.
  | { type: 'om_status'; workspace: string; session: string; kind: 'observing' | 'reflecting' | 'idle' }
  | { type: 'system'; workspace: string; session: string | null; kind: SystemEventKind }
  | { type: 'subagent_event'; workspace: string; session: string; kind: SubagentEventKind }
  // The session's plan file changed (the plan watcher, spec §5.3): full-
  // state replacement, idempotent — the plan pane renders it in place.
  | { type: 'plan_changed'; workspace: string; session: string; plan: Plan }
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
    return await invoke<CommandOutput>('tau_command', { command: cmd });
  } catch (e) {
    // The Rust side rejects with the deserialized ProtocolError: a plain
    // object carrying message (other/unsupported) or what (not_found) —
    // neither stringifies to anything useful on its own.
    const o = e as { message?: string; what?: string } | string;
    const msg = typeof o === 'string' ? o : o?.message ?? o?.what ?? JSON.stringify(o);
    throw new Error(msg, { cause: e });
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
