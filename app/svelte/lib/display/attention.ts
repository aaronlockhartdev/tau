// The attention fold (workbench P5): a GUI-side projection over state the
// core already tracks — no protocol change. A session needs you when a
// child is parked on the user.

import type { SessionState } from '../rules/sessions';
import { isWaitingOnUser } from './subagent-state';

export interface AttentionItem {
  session: string;
  workspace: string;
  title: string;
  reason: string;
  /// Millisecond timestamp for the elapsed readout; 0 when unknown.
  since: number;
}

export function attentionOf(sessions: Record<string, SessionState>): AttentionItem[] {
  const items: AttentionItem[] = [];
  for (const s of Object.values(sessions)) {
    if (s.meta.archived) continue;
    const ws = s.meta.workspace;
    const title = s.meta.title ?? s.meta.id;
    // A child parked on the user (spec §5.2: a GUI badge, not a wake) — the
    // same predicate the sidebar's badge label reads, so a child the tree
    // marks `user` is never left out of the bell's list.
    for (const a of s.subagents) {
      if (isWaitingOnUser(a.state)) {
        const who = sessions[a.child]?.meta.title ?? a.agent_type;
        items.push({
          session: s.meta.id,
          workspace: ws,
          title,
          reason: `${who} is waiting on you`,
          since: 0
        });
      }
    }
  }
  // Most recently changed first; unknown timestamps sink.
  return items.sort((a, b) => b.since - a.since);
}
