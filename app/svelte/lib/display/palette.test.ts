// The palette item builder: grouping and filtering, no DOM.
import { describe, expect, it } from 'vitest';
import { paletteItems, filterItems, type PaletteSource } from './palette';
import { seedState } from '../testing/mock-store.svelte.ts';
import type { SessionMeta } from '../wire/protocol';

const meta = (id: string, over: Partial<SessionMeta> = {}): SessionMeta => ({
  id,
  workspace: over.workspace ?? 'w1',
  title: over.title ?? null,
  parent: null,
  created: 1000,
  leaf: null,
  model: null,
  usage: null,
  archived: over.archived ?? false,
  ...over
});

function source(over: Partial<PaletteSource> = {}): PaletteSource {
  const sessions = {
    a: seedState(meta('a', { title: 'alpha' })),
    b: seedState(meta('b', { title: 'beta', workspace: 'w2' }))
  };
  return {
    sessions,
    current: 'a',
    focus: false,
    openSession: () => {},
    follow: () => {},
    newSession: () => {},
    openAttention: () => {},
    openFleet: () => {},
    toggleFocus: () => {},
    stop: () => {},
    archive: () => {},
    ...over
  };
}

describe('paletteItems', () => {
  it('groups sessions (last-modified order), crew, and the fixed actions', () => {
    const items = paletteItems(source());
    const groups = items.map((i) => i.group);
    // Sessions first, then crew (none for a bare session), then actions.
    expect(groups[0]).toBe('sessions');
    expect(groups[groups.length - 1]).toBe('actions');
    const labels = items.map((i) => i.label);
    expect(labels).toContain('alpha');
    expect(labels).toContain('New session');
    expect(labels).toContain('Fleet');
    expect(labels).toContain('Needs you');
    expect(labels).toContain('Focus mode');
  });

  it('excludes archived sessions', () => {
    const s = source({
      sessions: { a: seedState(meta('a', { title: 'alpha', archived: true })) }
    });
    expect(paletteItems(s).map((i) => i.label)).not.toContain('alpha');
  });

  it('lists the current session\'s crew as follow items', () => {
    const s = source({
      sessions: {
        a: seedState(meta('a'), {
          over: {
            subagents: [
              {
                handle: 'h1',
                child: 'c1',
                agent_type: 'researcher',
                context_mode: 'fresh',
                state: 'running',
                waiting_on: null,
                last_message: null,
                usage: null,
                resume_contract: null
              }
            ]
          }
        })
      }
    });
    const crew = paletteItems(s).filter((i) => i.group === 'crew');
    expect(crew).toHaveLength(1);
    expect(crew[0]!.label).toBe('researcher');
  });

  it('labels the focus toggle by current mode', () => {
    expect(paletteItems(source({ focus: true })).find((i) => i.label.includes('focus'))?.label).toBe(
      'Leave focus mode'
    );
  });

  it('wiring: openSession runs the session item', () => {
    let opened: string | null = null;
    const s = source({ openSession: (id: string) => (opened = id) });
    paletteItems(s)
      .find((i) => i.label === 'beta')
      ?.run();
    expect(opened).toBe('b');
  });
});

describe('filterItems', () => {
  const items = paletteItems(source());

  it('returns everything for an empty query', () => {
    expect(filterItems(items, '')).toHaveLength(items.length);
  });

  it('matches label or hint, case-insensitively', () => {
    const byLabel = filterItems(items, 'ALPHA');
    expect(byLabel.map((i) => i.label)).toEqual(['alpha']);
    const byHint = filterItems(items, 'w2');
    expect(byHint.map((i) => i.label)).toEqual(['beta']);
  });

  it('drops everything that does not match', () => {
    expect(filterItems(items, 'zzz-nope')).toHaveLength(0);
  });
});
