// The presentation mapping (F3): Entry in, presentable shape out — the
// per-kind label, icon, kv rows, and sections, testable without mounting a
// component.

import { describe, expect, it } from 'vitest';
import { present, type Shell } from './presentation';
import { Bot, File, Search, Terminal } from '@lucide/svelte';
import type { Diagnostic, Entry, MessageEntry } from '../wire/protocol';

const ctx = { sourceLabel: '', parentLabel: '', parentId: '' };

function shells(entry: Entry): Shell[] {
  const p = present(entry, ctx);
  if (!p) throw new Error('expected a presentation');
  return p.shells;
}

function cardOf(
  entry: Entry,
  over: { sourceLabel?: string; parentLabel?: string; parentId?: string } = {}
): Extract<Shell, { kind: 'card' }> {
  const p = present(entry, {
    sourceLabel: over.sourceLabel ?? '',
    parentLabel: over.parentLabel ?? '',
    parentId: over.parentId ?? ''
  });
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
        body: 'pondering'
      },
      {
        kind: 'card',
        header: null,
        cls: null,
        openKey: null,
        collapseWhenClosed: false,
        sections: [{ type: 'md', text: 'the answer' }],
        reveal: null,
        expando: null,
        intmark: null
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
    expect(c.intmark).toBe('interrupted');
  });

  it('a provider cutoff badges the card with what ended it', () => {
    const c = cardOf({ id: 'e1', kind: 'message', text: 'half a sentence', endedBy: 'length' });
    expect(c.cls).toBe('cutoff');
    expect(c.intmark).toBe('cut short · output cap');
  });

  it('a clean ending carries no badge, and an empty cut card still shows', () => {
    expect(cardOf({ id: 'e1', kind: 'message', text: 'done', endedBy: 'completed' }).intmark).toBeNull();
    const cut = cardOf({ id: 'e1', kind: 'message', text: '', endedBy: 'eof_without_done' });
    expect(cut.cls).toBe('cutoff');
    expect(cut.intmark).toBe('cut short · stream ended early');
  });

  it('a cut the user asked for shows one mark, whatever the provider said', () => {
    // The provider stopped at its cap and the user stopped the session: the
    // entry's kind carries the user's cut, so the card says it once (#307).
    const c = cardOf({
      id: 'e1',
      kind: 'interrupted',
      text: 'half a sentence',
      endedBy: 'length'
    });
    expect(c.cls).toBe('interrupted');
    expect(c.intmark).toBe('interrupted');
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

  it('#243: a split tool card renders ui_detail, not the model-facing output', () => {
    const s = shells({
      id: 'e1',
      kind: 'tool',
      name: 'write',
      args: { path: 'a.txt' },
      status: 'ok',
      output: 'wrote a.txt (7 bytes, 2 lines)',
      ui_detail: 'alpha\nbeta\n'
    })[0];
    if (s?.kind !== 'tool') throw new Error('expected a tool shell');
    // The card body is the detail channel; the model ack is not the card text.
    expect(s.output?.text).toBe('alpha\nbeta\n');
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
    expect(icon('bash')).toBe(Terminal);
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
      { type: 'md', text: 'the observation', dim: true },
      { type: 'kv', rows: [{ k: 'suggested', lines: ['do X'] }] },
      { type: 'md', text: 'thought deeply', cls: 'thinkbody', label: 'thinking' },
      { type: 'pre', text: 'some input', label: 'input' }
    ]);
  });

  it('an observation without details is still collapsible (body in the reveal)', () => {
    const c = cardOf({ id: 'e1', kind: 'om', text: 'plain' });
    expect(c.header?.caret).toBe(true);
    expect(c.collapseWhenClosed).toBe(true);
    expect(c.sections).toEqual([]);
    expect(c.reveal?.group).toBe(false);
    expect(c.reveal?.sections).toEqual([{ type: 'md', text: 'plain', dim: true }]);
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

  it('a user-sourced routed send renders a plain bubble, not a card', () => {
    const p = present(
      { id: 'e1', kind: 'user', text: 'steer me', source: 'user', lane: 'steering' },
      { sourceLabel: '', parentLabel: 'p1', parentId: 'p1' }
    );
    // Not the parent card, not a sub-agent card: the plain drain divider + bubble.
    expect(p?.shells.map((s) => s.kind)).toEqual(['divider', 'bubble']);
  });

  it('a sourced entry from the session parent renders as the parent', () => {
    const c = cardOf(
      { id: 'e1', kind: 'user', text: 'focus on edge cases', source: 'p1' },
      { sourceLabel: 'p1', parentLabel: 'Parent Session', parentId: 'p1' }
    );
    expect(c.header?.label).toBe('parent · Parent Session');
    expect(c.header?.cls).toBe('par');
  });

  it('a sourced entry from a non-parent renders as a sub-agent', () => {
    const c = cardOf(
      { id: 'e1', kind: 'user', text: 'report', source: 's2' },
      { sourceLabel: 'worker 2', parentLabel: 'Parent Session', parentId: 'p1' }
    );
    expect(c.header?.label).toBe('sub-agent · worker 2');
    expect(c.header?.cls).toBe('sub');
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
    expect(c.sections).toEqual([{ type: 'md', text: 'hello' }]);
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

describe('a diagnostic entry (ADR-0034)', () => {
  function diagnostic(d: Diagnostic): Entry {
    return { id: 'e1', kind: 'diagnostic', text: d.code, diagnostic: d };
  }

  function line(d: Diagnostic): string {
    const [shell] = shells(diagnostic(d));
    if (!shell || shell.kind !== 'stline') throw new Error('expected a state line');
    return shell.label;
  }

  it('composes the sentence from the typed fact, not a stored note', () => {
    expect(line({ code: 'gate_unavailable', model: 'dev/x', missing: ['context_window', 'max_tokens'] }))
      .toBe('context gate off for dev/x: context_window undeclared; max_tokens undeclared');
    expect(line({ code: 'gate_unavailable', model: 'dev/x', missing: ['context_room'] }))
      .toBe(
        'context gate off for dev/x: the declared window leaves no room once the output cap and the margin are taken'
      );
    expect(line({ code: 'turn_cutoff', model: 'dev/x', ended_by: 'eof_without_done', blocks: 2 }))
      .toBe('provider cut the turn (stream ended early) after 2 block(s)');
    expect(line({ code: 'om_stalled', pending: 24000, activation: 24000 }))
      .toBe('compaction stalled: 24.0k pending at activation 24.0k');
  });

  // A windowed transcript holds snapshot metadata until a paged read hydrates
  // the entry; the skeleton has no payload, so the card falls back to the
  // preview, which the core sets to the code name.
  it('an unhydrated row renders the code its preview carries', () => {
    const skeleton: Entry = { id: 'e1', kind: 'diagnostic', text: 'gate_unavailable' };
    const [shell] = shells(skeleton);
    if (!shell || shell.kind !== 'stline') throw new Error('expected a state line');
    expect(shell.label).toBe('gate_unavailable');
  });

  it('a model change names its predecessor, or says there is none', () => {
    expect(line({ code: 'model_changed', from: 'a', to: 'b' })).toBe('model: a → b');
    // A config-level change: the session never recorded a model to leave.
    expect(line({ code: 'model_changed', to: 'b' })).toBe('model: b (no recorded predecessor)');
  });

  // The two gates are different things: the completion gate bounces the
  // model with a check, the context gate refuses a request.
  it('the completion gate names the check, the attempt, and the output head', () => {
    expect(line({ code: 'gate_attempt', command: 'cargo nextest run', retries: 1, max: 2, output: 'test failed\nsecond line' }))
      .toBe('completion gate: cargo nextest run failed (repair 1 of 2): test failed');
    expect(line({ code: 'gate_failed', command: 'cargo nextest run', retries: 2 }))
      .toBe('completion gate: cargo nextest run failed after 2 repairs — marked gate_failed');
    expect(line({ code: 'context_overflow', prompt: 120000, budget: 110592 }))
      .toBe('context gate refused: prompt 120.0k over budget 110.6k');
  });

  // The negative: a code from a newer core still renders, by its own name.
  it('an unknown code renders its own name', () => {
    expect(line({ code: 'tool_stalled' })).toBe('tool_stalled');
  });
});
