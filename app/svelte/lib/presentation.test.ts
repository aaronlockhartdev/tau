// The presentation mapping (F3): Entry in, presentable shape out — the
// per-kind label, icon, kv rows, and sections, testable without mounting a
// component.

import { describe, expect, it } from 'vitest';
import { md } from './markdown';
import { present, type Shell } from './presentation';
import { Bot, Check, File, Search, Terminal } from '@lucide/svelte';
import type { Entry, MessageEntry } from './protocol';

const ctx = { sourceLabel: '', parentLabel: '' };

function shells(entry: Entry): Shell[] {
  const p = present(entry, ctx);
  if (!p) throw new Error('expected a presentation');
  return p.shells;
}

function cardOf(
  entry: Entry,
  over: { sourceLabel?: string; parentLabel?: string } = {}
): Extract<Shell, { kind: 'card' }> {
  const p = present(entry, { sourceLabel: over.sourceLabel ?? '', parentLabel: over.parentLabel ?? '' });
  if (!p) throw new Error('expected a presentation');
  const s = p.shells.find((s) => s.kind === 'card');
  if (!s || s.kind !== 'card') throw new Error('expected a card shell');
  return s;
}

const msg = (over: Partial<MessageEntry> = {}): Entry => ({
  id: 'e1',
  kind: 'message',
  text: 'the answer',
  ...over
});

describe('present', () => {
  it('a message with reasoning is a thinking line plus a card', () => {
    const p = present(
      {
        id: 'e1',
        kind: 'message',
        text: 'the answer',
        reasoning: 'pondering',
        usage: { input_tokens: 1200, output_tokens: 340, total_tokens: 1540, cached_prompt_tokens: 0 }
      },
      ctx
    );
    expect(p).not.toBeNull();
    expect(p?.shells).toEqual([
      {
        kind: 'think',
        label: 'thinking',
        meta: '1.2k in · 340 out',
        openKey: 'think',
        body: md('pondering')
      },
      {
        kind: 'card',
        header: null,
        cls: null,
        openKey: null,
        collapseWhenClosed: false,
        sections: [{ type: 'md', html: md('the answer') }],
        reveal: null,
        expando: null,
        intmark: false
      }
    ]);
  });

  it('a message without reasoning is just the card', () => {
    const p = present(msg(), ctx);
    expect(p?.shells).toHaveLength(1);
    expect(p?.shells[0]?.kind).toBe('card');
  });

  it('an interrupted entry carries the intmark even with empty text', () => {
    const c = cardOf({ id: 'e1', kind: 'interrupted', text: '' });
    expect(c.cls).toBe('interrupted');
    expect(c.intmark).toBe(true);
  });

  it('a bash tool call is a chip with summary, kv rows and the combined output', () => {
    const s = shells({
      id: 'e1',
      kind: 'tool',
      name: 'bash',
      args: { command: 'ls -la' },
      status: 'ok',
      output: 'file1\nfile2'
    })[0];
    expect(s).toEqual({
      kind: 'tool',
      icon: Terminal,
      label: 'bash',
      summary: 'ls -la',
      status: 'ok',
      openKey: 'tool',
      kv: [{ k: 'command', lines: ['ls -la'] }],
      output: { text: 'ls -la\n\nfile1\nfile2', closed: 'ls -la\n\nfile1\nfile2', long: false }
    });
  });

  it('a long read output collapses to the TAIL preview', () => {
    const body = 'HEAD' + 'y'.repeat(300) + 'TAIL';
    const s = shells({
      id: 'e1',
      kind: 'tool',
      name: 'read',
      args: { path: 'a.txt' },
      status: 'ok',
      output: body
    })[0];
    if (s?.kind !== 'tool') throw new Error('expected a tool shell');
    expect(s.output?.long).toBe(true);
    expect(s.output?.closed).toBe(' …' + body.slice(-200));
    expect(s.output?.text).toBe(body);
  });

  it('a long non-tail tool output keeps the HEAD preview', () => {
    const body = 'HEAD' + 'y'.repeat(300) + 'TAIL';
    const s = shells({
      id: 'e1',
      kind: 'tool',
      name: 'recall',
      args: { query: 'q' },
      status: 'ok',
      output: body
    })[0];
    if (s?.kind !== 'tool') throw new Error('expected a tool shell');
    expect(s.output?.closed).toBe(body.slice(0, 200) + ' …');
  });

  it('carries the path arg onto the tool shell for output highlighting', () => {
    const s = shells({
      id: 'e1',
      kind: 'tool',
      name: 'edit',
      args: { path: 'src/a.ts' },
      status: 'ok',
      output: 'edited src/a.ts (lines 1–1 of 2)\n+ abc x'
    })[0];
    if (s?.kind !== 'tool') throw new Error('expected a tool shell');
    expect(s.path).toBe('src/a.ts');
  });

  it('uses the path arg for the chip summary of file tools (not JSON)', () => {
    const s = shells({
      id: 'e1',
      kind: 'tool',
      name: 'read',
      args: { path: 'src/a.ts' },
      status: 'ok',
      output: 'x'
    })[0];
    if (s?.kind !== 'tool') throw new Error('expected a tool shell');
    expect(s.summary).toBe('src/a.ts');
  });

  it('a running tool has no output section', () => {
    const s = shells({ id: 'e1', kind: 'tool', name: 'read', status: 'running' })[0];
    if (s?.kind !== 'tool') throw new Error('expected a tool shell');
    expect(s.status).toBe('running');
    expect(s.output).toBeNull();
  });

  it('maps tool names to icons', () => {
    const icon = (name: string) => {
      const s = shells({ id: 'e1', kind: 'tool', name, status: 'running' })[0];
      if (s?.kind !== 'tool') throw new Error('expected a tool shell');
      return s.icon;
    };
    expect(icon('read')).toBe(File);
    expect(icon('subagent_spawn')).toBe(Bot);
    expect(icon('parent_notify')).toBe(Bot);
    expect(icon('recall')).toBe(Search);
    expect(icon('task_create')).toBe(Check);
    expect(icon('bash')).toBe(Terminal);
  });

  it('a task card labels the event and reveals the payload rows without the event key', () => {
    const c = cardOf({
      id: 'e1',
      kind: 'task',
      text: 'x',
      payload: { event: 'created', id: 't1', title: 'Ship the report', steps: [], criteria: [] }
    });
    expect(c.header?.label).toBe('task: Ship the report');
    expect(c.header?.open).toBe('task');
    expect(c.header?.singleClick).toBe(true);
    expect(c.collapseWhenClosed).toBe(true);
    const reveal = c.reveal;
    expect(reveal?.group).toBe(false);
    expect(reveal?.sections).toEqual([
      {
        type: 'kv',
        rows: [
          { k: 'id', lines: ['t1'] },
          { k: 'title', lines: ['Ship the report'] },
          { k: 'steps', lines: [] },
          { k: 'criteria', lines: [] }
        ]
      }
    ]);
  });

  it('an observation card is collapsible with its body in the reveal', () => {
    const c = cardOf({
      id: 'e1',
      kind: 'om',
      text: 'the observation',
      model: 'om-1',
      thinking: 'thought deeply',
      suggestedResponse: 'do X',
      input: 'some input'
    });
    expect(c.cls).toBe('obs');
    expect(c.header?.label).toBe('observation');
    expect(c.header?.meta).toBe('om-1');
    expect(c.header?.caret).toBe(true);
    expect(c.header?.open).toBe('obs');
    expect(c.header?.singleClick).toBe(true);
    expect(c.collapseWhenClosed).toBe(true);
    expect(c.sections).toEqual([]);
    expect(c.reveal?.group).toBe(false);
    expect(c.reveal?.sections).toEqual([
      { type: 'md', html: md('the observation'), dim: true },
      { type: 'kv', rows: [{ k: 'suggested', lines: ['do X'] }] },
      { type: 'md', html: md('thought deeply'), cls: 'thinkbody', label: 'thinking' },
      { type: 'pre', text: 'some input', label: 'input' }
    ]);
  });

  it('an observation without details is still collapsible (body in the reveal)', () => {
    const c = cardOf({ id: 'e1', kind: 'om', text: 'plain' });
    expect(c.header?.caret).toBe(true);
    expect(c.collapseWhenClosed).toBe(true);
    expect(c.sections).toEqual([]);
    expect(c.reveal?.group).toBe(false);
    expect(c.reveal?.sections).toEqual([{ type: 'md', html: md('plain'), dim: true }]);
  });

  it('a plain user entry is a bubble', () => {
    expect(present({ id: 'e1', kind: 'user', text: 'hi' }, ctx)?.shells).toEqual([
      { kind: 'bubble', text: 'hi' }
    ]);
  });

  it('a steering delivery carries its lane divider', () => {
    expect(present({ id: 'e1', kind: 'user', text: 'do X', lane: 'steering' }, ctx)?.shells).toEqual(
      [
        { kind: 'divider', label: 'steering · next tool boundary', tint: 'drain' },
        { kind: 'bubble', text: 'do X' }
      ]
    );
  });

  it('a follow-up delivery carries its lane divider', () => {
    expect(
      present({ id: 'e1', kind: 'user', text: 'then do Y', lane: 'follow_up' }, ctx)?.shells
    ).toEqual([
      { kind: 'divider', label: 'follow-up · after work completes', tint: 'drain' },
      { kind: 'bubble', text: 'then do Y' }
    ]);
  });

  it('a force send has no divider', () => {
    expect(present({ id: 'e1', kind: 'user', text: 'stop', lane: 'force' }, ctx)?.shells).toEqual(
      [{ kind: 'bubble', text: 'stop' }]
    );
  });

  it('a compaction record leads with its divider', () => {
    const p = present({ id: 'e1', kind: 'om', text: 'plain' }, ctx);
    expect(p?.shells[0]).toEqual({
      kind: 'divider',
      label: 'compaction · observation log updated',
      tint: 'compact'
    });
    expect(p?.shells).toHaveLength(2);
  });

  it('a skill user entry is a card that collapses to a preview with an expando', () => {
    const body = 'y'.repeat(250);
    const c = cardOf({
      id: 'e1',
      kind: 'user',
      text: body,
      skill: { name: 'alpha', location: '/skills/alpha' }
    });
    expect(c.header?.label).toBe('skill · alpha');
    expect(c.header?.open).toBe('skill');
    expect(c.header?.singleClick).toBe(false);
    expect(c.expando).toEqual({
      openKey: 'skill',
      whenOpen: '▾ hide',
      whenClosed: '▸ full (250 chars)'
    });
    const text = c.sections[0];
    expect(text?.type).toBe('text');
    if (text?.type === 'text') expect(text.whenClosed).toBe(body.slice(0, 200) + ' …');
  });

  it('a sourced user entry renders the decoded report as prose and kv', () => {
    const c = cardOf(
      {
        id: 'e1',
        kind: 'user',
        text: 'raw',
        source: 's2',
        msg: { prose: 'Done!', kv: [{ k: 'files', lines: ['a.txt'] }] }
      },
      { sourceLabel: 'worker 2', parentLabel: '' }
    );
    expect(c.header?.label).toBe('sub-agent · worker 2');
    expect(c.header?.cls).toBe('sub');
    expect(c.sections).toEqual([
      { type: 'text', text: 'Done!', dim: true },
      { type: 'kv', rows: [{ k: 'files', lines: ['a.txt'] }], block: true }
    ]);
  });

  it('a parent-bound user entry splits the JSON payload here', () => {
    const c = cardOf(
      { id: 'e1', kind: 'user', text: 'all good {"ok": true}' },
      { sourceLabel: '', parentLabel: 'p1' }
    );
    expect(c.header?.label).toBe('parent · p1');
    expect(c.header?.cls).toBe('par');
    expect(c.sections).toEqual([
      { type: 'text', text: 'all good', dim: true },
      { type: 'kv', rows: [{ k: 'ok', lines: ['true'] }], block: true }
    ]);
  });

  it('a sub-agent state event is a quiet state line', () => {
    expect(
      present(
        {
          id: 'e1',
          kind: 'subagent',
          text: 'x',
          payload: { event: 'state', state: 'idle', waiting_on: 'parent' }
        },
        ctx
      )?.shells
    ).toEqual([{ kind: 'stline', icon: Bot, label: 'state: idle · parent' }]);
  });

  it('labels a spawn event by the parent, but not in the child\'s own session', () => {
    const entry: Entry = {
      id: 'e1',
      kind: 'subagent',
      text: 'x',
      payload: {
        event: 'spawn',
        type: 'worker',
        brief: 'b',
        context_mode: 'fresh',
        parent: 's1',
        call: 'c1'
      }
    };
    expect(cardOf(entry).header?.label).toBe('sub-agent');
    expect(cardOf(entry, { parentLabel: 'p1' }).header?.label).toBe('spawn');
  });

  it('a model system entry is a state line; other system entries are cards', () => {
    expect(
      present({ id: 'e1', kind: 'system', text: 'model: acme/x-1' }, ctx)?.shells
    ).toEqual([{ kind: 'stline', icon: Bot, label: 'model: acme/x-1' }]);
    const c = cardOf({ id: 'e1', kind: 'system', text: 'something else' });
    expect(c.header?.label).toBe('system');
    expect(c.header?.icon).toBe(Terminal);
  });

  it('a spawn snapshot is a labelled card', () => {
    const c = cardOf({ id: 'e1', kind: 'spawn-snapshot', text: 'the log' });
    expect(c.header?.label).toBe('spawn snapshot');
  });

  it('an unknown kind degrades to a plain text card, not a throw', () => {
    const c = cardOf({ id: 'e1', kind: 'mystery', text: 'hello' });
    expect(c.header).toBeNull();
    expect(c.sections).toEqual([{ type: 'md', html: md('hello') }]);
  });

  it('an unknown kind without text renders nothing', () => {
    expect(present({ id: 'e1', kind: 'mystery' }, ctx)).toBeNull();
  });

  it('a fully empty entry renders nothing', () => {
    expect(present({ id: 'e1', kind: 'message', text: '' }, ctx)).toBeNull();
    expect(present({ id: 'e1', kind: 'user', text: '   ' }, ctx)).toBeNull();
    expect(present({ id: 'e1', kind: 'om', text: '' }, ctx)).toBeNull();
  });
});
