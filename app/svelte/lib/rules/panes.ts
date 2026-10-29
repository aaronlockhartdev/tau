// The pane view state (spec §9) and the tab-strip multiselect (B2): the
// store keeps the state, this module owns the pure rules over it.

import { nextSelection } from './selection';

export interface PaneState {
  // The center column's view: the transcript ('chat') or the file viewer
  // ('files'). Open files are separate state, so 'chat' keeps them alive.
  center: 'chat' | 'files';
  ltab: 'files' | 'sessions';
  // The right pane's tab: the sub-agents panel, or the session's plan
  // (spec §5.3) — the plan pane that replaced the task panel.
  rtab: 'subs' | 'plan';
  // F3: the right-pane history section (done/failed/stopped rows) starts
  // collapsed; per-row badges carry the exact state, so no filter exists.
  historyOpen: boolean;
  // null = the default view (the group containing the active session
  // expanded); an array = the explicit set the user has toggled.
  openGroups: string[] | null;
  renamingId: string | null;
  archOpen: boolean;
  selSub: string | null;
  // The session-tree multiselect (cmd/ctrl toggle, shift range): the
  // selected ids and the row a shift range extends from.
  selected: string[];
  selAnchor: string | null;
}

export function defaultPane(): PaneState {
  return {
    center: 'chat',
    ltab: 'sessions',
    rtab: 'subs',
    historyOpen: false,
    openGroups: null,
    renamingId: null,
    archOpen: false,
    selSub: null,
    selected: [],
    selAnchor: null
  };
}

export function paneOf(panes: Record<string, PaneState>, ws: string | null): PaneState | null {
  if (ws === null) return null;
  return panes[ws] ?? null;
}

export interface TabSelection {
  selected: string[];
  anchor: string | null;
}

// The tab strip's multiselect (B2): a thin adapter over nextSelection —
// the shared rule; the tab strip keeps the selection (and returns the
// same object) when a shift anchor is stale.
export function applyTabSelect(
  prev: TabSelection,
  workspaceIds: string[],
  ws: string,
  e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }
): TabSelection {
  const selected = nextSelection(workspaceIds, prev.selected, prev.anchor, ws, {
    shift: e.shiftKey,
    multi: e.metaKey || e.ctrlKey,
    shiftFallback: 'keep'
  });
  if (selected === null) return prev;
  return { selected, anchor: e.shiftKey ? prev.anchor : ws };
}
