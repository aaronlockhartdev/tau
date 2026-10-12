// The command palette's item model (workbench P6): a pure builder over a
// store snapshot plus the actions the palette may fire, so the grouping and
// filtering are unit-testable without mounting the overlay.

import type { SessionState } from './sessions';
import type { SubagentInfo } from './protocol';

export interface PaletteItem {
  group: 'sessions' | 'crew' | 'actions';
  label: string;
  hint?: string;
  run: () => void;
}

export interface PaletteSource {
  sessions: Record<string, SessionState>;
  current: string | null;
  focus: boolean;
  openSession: (sid: string) => void;
  follow: (handle: string) => void;
  newSession: () => void;
  openAttention: () => void;
  openFleet: () => void;
  toggleFocus: () => void;
  stop: () => void;
  archive: () => void;
}

export function paletteItems(s: PaletteSource): PaletteItem[] {
  const items: PaletteItem[] = [];
  const sessions = Object.values(s.sessions)
    .filter((x) => !x.meta.archived)
    .sort((a, b) => b.mru - a.mru);
  for (const x of sessions) {
    items.push({
      group: 'sessions',
      label: x.meta.title ?? x.meta.id,
      hint: x.meta.workspace,
      run: () => s.openSession(x.meta.id)
    });
  }
  const crew: SubagentInfo[] = s.current ? (s.sessions[s.current]?.subagents ?? []) : [];
  for (const a of crew) {
    items.push({
      group: 'crew',
      label: a.agent_type,
      hint: a.state,
      run: () => s.follow(a.handle)
    });
  }
  items.push(
    { group: 'actions', label: 'New session', run: s.newSession },
    { group: 'actions', label: 'Needs you', run: s.openAttention },
    { group: 'actions', label: 'Fleet', run: s.openFleet },
    {
      group: 'actions',
      label: s.focus ? 'Leave focus mode' : 'Focus mode',
      run: s.toggleFocus
    },
    { group: 'actions', label: 'Stop', run: s.stop },
    { group: 'actions', label: 'Archive current session', run: s.archive }
  );
  return items;
}

export function filterItems(items: PaletteItem[], query: string): PaletteItem[] {
  const q = query.trim().toLowerCase();
  if (!q) return items;
  return items.filter(
    (i) => i.label.toLowerCase().includes(q) || (i.hint ?? '').toLowerCase().includes(q)
  );
}
