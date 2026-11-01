// The GUI's projection of the wire's tagged child state (#346, spec §5.2).
//
// The crate sends `SubagentState` as a tagged enum (the core's `ChildState`,
// ADR-0006's mirror), so a badge is `state.type` and its detail rides the same
// arm: `idle` carries `waiting_on`, `failed` its `reason`, `stopped` its
// `by`. The badge, the row's state, and the tree's counts all read that one
// value, and this module is the one place the tag is narrowed — plus the one
// that turns a badge back into the wire type for the rows the core no longer
// lists.
import type { SubagentState, WaitingOn } from '../wire/protocol';

// The row's badge: the tag, with an `other` state naming itself (the
// forward-compat arm renders from the spelling the wire sent).
export type ChildBadge = SubagentState['type'];

export function badgeOf(state: SubagentState): ChildBadge {
  return state.type;
}

// The badge's label text.
export function badgeLabel(state: SubagentState): string {
  if (state.type === 'other') {
    // A tag the GUI has never seen still needs a badge, and an empty raw
    // spells none.
    return state.raw === '' ? 'unknown' : state.raw;
  }
  return state.type;
}

// The declared wait a state carries: only `idle` declares one.
export function waitingOnOf(state: SubagentState): WaitingOn | null {
  return state.type === 'idle' ? state.waiting_on : null;
}

// Whose attention the child has: an `awaiting` child is asking the user (the
// `QUESTION:` marker, spec §5.2), an `idle` child declares its target. The
// tree's counts and the pane's filter read the state through this, so the
// tag's two arms stay in one place (#346).
export function isWaitingOnUser(state: SubagentState): boolean {
  return state.type === 'awaiting' || (state.type === 'idle' && state.waiting_on === 'user');
}

// The badge's hint beside the label: an `idle` child names its declared
// target, an `awaiting` child asks the user (spec §5.2).
export function waitingHintOf(state: SubagentState): WaitingOn | null {
  return state.type === 'awaiting' ? 'user' : waitingOnOf(state);
}

// The fact a notice of this state carries, in the shape the row records. A
// row's state is a value, so it needs a target: a child parked with no
// declared wait is parked on its delegating parent, the rule the wire applies
// to a notice that declares no target (states.rs).
export function stateOfBadge(badge: ChildBadge, waitingOn: WaitingOn | null): SubagentState {
  switch (badge) {
    case 'running':
      return { type: 'running' };
    case 'awaiting':
      return { type: 'awaiting' };
    case 'idle':
      return { type: 'idle', waiting_on: waitingOn ?? 'parent' };
    case 'done':
      // The report is the notify record's, never the row's.
      return { type: 'done', output: null };
    default:
      // A `failed` badge names no reason, a `stopped` badge no storer:
      // the badge renders by its name and the unobserved fact is not
      // invented (the decoder's rule for a truncated frame).
      return { type: 'other', raw: badge };
  }
}

// The wake kind of a `notified` event is the core's own vocabulary (done /
// failed / waiting / stopped), not the state's: a wake names what the parent
// was told, and only a terminal wake settles the child's badge.
const WAKE_BADGE: Record<string, ChildBadge> = {
  done: 'done',
  failed: 'failed',
  waiting: 'idle',
  stopped: 'stopped',
};

// A wake settles the badge it names: `done` and `failed` are terminal, the
// core's `waiting` re-parks the child, and a `stopped` wake says the child
// stopped — a word the sidebar badge tells truthfully. A word this build does
// not know is no state it can name, so it reads `other`.
export function badgeOfWake(wake: string): ChildBadge {
  return WAKE_BADGE[wake] ?? 'other';
}

// The wake's state for a mirror row whose state event never arrived (a lost
// batch — the stream's self-heal is the next snapshot). A `done` wake carries
// the report, a `failed` wake states the reason in its text, and a `waiting`
// wake declares no target — it re-parks a child where it was, so the caller
// hands in the target the row already declares; a row with no observed wait is
// parked on the delegating parent, the rule `settle_rest` applies when the
// notify parks a child that ended without a question (lifecycle.rs). A `stopped` wake names no storer — the stop event does,
// and `stopped` sends no wake at all (spec §5.2) — so that badge keeps the
// `other` arm rather than inventing a `who`.
export function stateOfWake(
  wake: string,
  text: string | null,
  output: unknown,
  waitingOn: WaitingOn | null,
): SubagentState {
  switch (wake) {
    case 'done':
      return { type: 'done', output: output ?? null };
    case 'failed':
      return { type: 'failed', reason: text ?? '' };
    case 'waiting':
      return { type: 'idle', waiting_on: waitingOn ?? 'parent' };
    default:
      return { type: 'other', raw: wake };
  }
}
