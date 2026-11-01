// The fixture builders for the wire's tagged child state (#346). The wire
// type is a tagged enum whose arms carry their own fields, so a fixture names
// the badge and the arm's field as siblings — `{ state: 'idle', waiting_on:
// 'user' }` — and this module assembles the arm. An arm field a fixture leaves
// out takes the default written here, so a test reads as the badge under test.
import type {
  StoppedBy,
  SubagentInfo,
  SubagentState,
  WaitingOn
} from '../wire/protocol';
import type { ChildBadge } from '../display/subagent-state';

// The flat form: a badge plus the arm fields as siblings.
export interface StateFixture {
  state?: ChildBadge;
  waiting_on?: WaitingOn;
  reason?: string;
  by?: StoppedBy;
  output?: unknown;
  raw?: string;
}

// One state from the flat form.
export function st(fix: StateFixture = {}): SubagentState {
  const { state = 'idle', waiting_on, reason, by, output, raw } = fix;
  switch (state) {
    case 'idle':
      return { type: 'idle', waiting_on: waiting_on ?? 'parent' };
    case 'done':
      return { type: 'done', output: output ?? null };
    case 'failed':
      return { type: 'failed', reason: reason ?? 'provider 502' };
    case 'stopped':
      return { type: 'stopped', by: by ?? 'user' };
    case 'other':
      return { type: 'other', raw: raw ?? 'parked' };
    default:
      return { type: state };
  }
}

// A crew mirror row (the shape `subagent_event` and `live.subagents` carry):
// the flat state plus the row's own fields, applied last.
export type SubFixture = Omit<Partial<SubagentInfo>, 'state' | 'waiting_on'> & StateFixture;

export function subInfo(child: string, fix: SubFixture = {}): SubagentInfo {
  const { state, waiting_on, reason, by, output, raw, ...rest } = fix;
  const stated = st({ state, waiting_on, reason, by, output, raw });
  return {
    handle: child,
    child,
    agent_type: 'general',
    context_mode: 'fresh',
    state: stated,
    last_message: null,
    usage: null,
    resume_contract: null,
    ...rest
  };
}

// A snapshot's `own_state` from the flat form (#221's field): the tag the wire
// sends, its detail riding the same arm.
export function ownState(fix: StateFixture = {}): SubagentState {
  return st(fix);
}
