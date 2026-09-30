// @vitest-environment jsdom
// One transcript entry: the user bubble, the sourced/parent report cards,
// the reasoning line, the tool chip (expand, long-output expando, running
// spinner), the observation card, and the quiet system/state lines.
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { Entry } from '../lib/protocol';
import EntryCard from './EntryCard.svelte';
import { resetMockStore } from '../lib/testing/mock-store.svelte.ts';

vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const user = userEvent.setup();

// entryOpen is a Map on the shared mock store: reset it per test so the
// expansion state one test toggled does not seed the next one's mount.
beforeEach(() => {
  resetMockStore();
});
type MountOver = Partial<{
  batch: { index: number; size: number };
  sourceLabel: string;
  parentLabel: string;
  turn: 'you' | 'agent' | '';
}>;

function mount(entry: Entry, over: MountOver = {}) {
  return render(EntryCard, {
    props: {
      entry,
      batch: over.batch,
      heightKey: 'h1',
      sourceLabel: over.sourceLabel ?? '',
      parentLabel: over.parentLabel ?? '',
      turn: over.turn ?? ''
    }
  });
}

describe('EntryCard', () => {
  it('a plain user entry renders a bubble', () => {
    mount({ id: 'e1', kind: 'user', text: 'hi there' });
    expect(screen.getByText('hi there').closest('.bubble')).not.toBeNull();
  });

  it('a sourced user entry renders the sub-agent report card', () => {
    mount(
      {
        id: 'e1',
        kind: 'user',
        text: 'raw',
        source: 's2',
        msg: { prose: 'Done!', kv: [{ k: 'files', lines: ['a.txt'] }] }
      },
      { sourceLabel: 'worker 2' }
    );
    expect(screen.getByText('sub-agent · worker 2')).toBeInTheDocument();
    expect(screen.getByText('Done!')).toBeInTheDocument();
    expect(screen.getByText('files:')).toBeInTheDocument();
    expect(screen.getByText('a.txt')).toBeInTheDocument();
  });

  it('a parent-bound user entry splits the JSON payload into prose and kv', () => {
    mount(
      { id: 'e1', kind: 'user', text: 'all good {"ok": true}' },
      { parentLabel: 'p1' }
    );
    expect(screen.getByText('parent · p1')).toBeInTheDocument();
    expect(screen.getByText('all good')).toBeInTheDocument();
    expect(screen.getByText('ok:')).toBeInTheDocument();
    expect(screen.getByText('true')).toBeInTheDocument();
  });

  it('a message with reasoning shows the thinking line open by default, with usage', async () => {
    mount({
      id: 'e1',
      kind: 'message',
      text: 'the answer',
      reasoning: 'pondering deeply',
      usage: { input_tokens: 1200, output_tokens: 340, total_tokens: 1540, cached_prompt_tokens: 0 }
    });
    await tick();
    expect(screen.getByText('the answer')).toBeInTheDocument();
    const think = screen.getByRole('button', { name: /thinking/ });
    expect(think).toHaveClass('think');
    expect(think).toHaveTextContent('1.2k in · 340 out');
    expect(screen.getByText('pondering deeply')).toBeInTheDocument();
  });

  it('clicking the thinking line collapses the reasoning', async () => {
    mount({
      id: 'e1',
      kind: 'message',
      text: 'the answer',
      reasoning: 'pondering deeply'
    });
    await tick();
    await user.click(screen.getByRole('button', { name: /thinking/ }));
    expect(screen.queryByText('pondering deeply')).toBeNull();
  });

  it('a finished tool chip shows name, summary and the ok mark; clicking expands args and output', async () => {
    mount({
      id: 'e1',
      kind: 'tool',
      name: 'bash',
      args: { command: 'ls -la' },
      status: 'ok',
      output: 'file1\nfile2'
    });
    const chip = screen.getByRole('button', { name: /bash/ });
    expect(chip).toHaveTextContent('ls -la');
    expect(chip).toHaveTextContent('✓');
    expect(screen.queryByText('command:')).toBeNull();
    await user.click(chip);
    expect(screen.getByText('command:')).toBeInTheDocument();
    expect(screen.getByText('ls -la', { selector: '.v' })).toBeInTheDocument();
    expect(screen.getByText('ls -la file1 file2')).toBeInTheDocument();
  });

  it('a running tool shows a spinner and no status mark', () => {
    const { container } = mount({
      id: 'e1',
      kind: 'tool',
      name: 'read',
      status: 'running'
    });
    expect(container.querySelector('.spin')).not.toBeNull();
    expect(screen.queryByText('✓')).toBeNull();
    expect(screen.queryByText('✗')).toBeNull();
  });

  it('a long tool output is previewed and the expando reveals the full text', async () => {
    mount({
      id: 'e1',
      kind: 'tool',
      name: 'bash',
      args: { command: 'cat big.txt' },
      status: 'ok',
      output: 'x'.repeat(250)
    });
    await user.click(screen.getByRole('button', { name: /bash/ }));
    expect(screen.queryByText('cat big.txt ' + 'x'.repeat(250))).toBeNull();
    const expando = screen.getByRole('button', { name: /full output/ });
    expect(expando).toHaveTextContent('263 chars');
    await user.click(expando);
    expect(screen.getByText('cat big.txt ' + 'x'.repeat(250))).toBeInTheDocument();
  });

  it('an observation card expands to its suggested, thinking and input sections', async () => {
    mount({
      id: 'e1',
      kind: 'om',
      text: 'the observation',
      model: 'om-1',
      thinking: 'thought deeply',
      suggestedResponse: 'do X',
      input: 'some input'
    });
    expect(screen.getByText('observation')).toBeInTheDocument();
    expect(screen.getByText('om-1')).toBeInTheDocument();
    expect(screen.queryByText('suggested:')).toBeNull();
    await user.dblClick(screen.getByRole('button', { name: /observation/ }));
    expect(screen.getByText('suggested:')).toBeInTheDocument();
    expect(screen.getByText('do X')).toBeInTheDocument();
    expect(screen.getByText('thought deeply')).toBeInTheDocument();
    expect(screen.getByText('some input')).toBeInTheDocument();
  });

  it('a sub-agent state event renders a quiet state line', () => {
    mount({
      id: 'e1',
      kind: 'subagent',
      text: JSON.stringify({ event: 'state', state: 'idle', waiting_on: 'parent' }),
      payload: { event: 'state', state: 'idle', waiting_on: 'parent' }
    });
    expect(screen.getByText('state: idle · parent')).toBeInTheDocument();
  });

  it('a sub-agent spawn event renders a card with the payload rows', () => {
    mount({
      id: 'e1',
      kind: 'subagent',
      text: JSON.stringify({
        event: 'spawn',
        type: 'worker',
        brief: 'do the thing',
        context_mode: 'fresh',
        parent: 's1',
        call: 'call-1'
      }),
      payload: {
        event: 'spawn',
        type: 'worker',
        brief: 'do the thing',
        context_mode: 'fresh',
        parent: 's1',
        call: 'call-1'
      }
    });
    expect(screen.getByText('spawn')).toBeInTheDocument();
    expect(screen.getByText('brief:')).toBeInTheDocument();
    expect(screen.getByText('do the thing')).toBeInTheDocument();
  });

  it('a task card labels the event and expands to its payload', async () => {
    mount({
      id: 'e1',
      kind: 'task',
      text: JSON.stringify({
        event: 'created',
        id: 't1',
        title: 'Ship the report',
        steps: [],
        criteria: []
      }),
      payload: {
        event: 'created',
        id: 't1',
        title: 'Ship the report',
        steps: [],
        criteria: []
      }
    });
    const head = screen.getByRole('button', { name: /task: Ship the report/ });
    expect(head).toHaveTextContent('▸');
    await user.click(head);
    expect(screen.getByText('id:')).toBeInTheDocument();
    expect(screen.getByText('t1')).toBeInTheDocument();
  });

  it('a model system entry renders as a quiet state line', () => {
    mount({ id: 'e1', kind: 'system', text: 'model: acme/x-1' });
    expect(screen.getByText('model: acme/x-1')).toBeInTheDocument();
  });

  it('an interrupted entry carries the interrupted mark', () => {
    mount({ id: 'e1', kind: 'interrupted', text: 'partial' });
    expect(screen.getByText('partial')).toBeInTheDocument();
    expect(screen.getByText('⚡ interrupted')).toBeInTheDocument();
  });

  it('a skill user entry shows the preview and dblclick reveals the full text', async () => {
    const body = 'y'.repeat(250);
    mount({
      id: 'e1',
      kind: 'user',
      text: body,
      skill: { name: 'alpha', location: '/skills/alpha' }
    });
    expect(screen.getByText('skill · alpha')).toBeInTheDocument();
    expect(screen.queryByText(body)).toBeNull();
    await user.dblClick(screen.getByRole('button', { name: /skill · alpha/ }));
    expect(screen.getByText(body)).toBeInTheDocument();
  });

  it('a spawn snapshot renders its body', () => {
    const { container } = mount({ id: 'e1', kind: 'spawn-snapshot', text: 'spawned with the brief' });
    expect(container.querySelector('.wrap')).not.toBeNull();
    expect(screen.getByText('spawn snapshot')).toBeInTheDocument();
    expect(screen.getByText('spawned with the brief')).toBeInTheDocument();
  });

  it('a fully empty entry renders nothing', () => {
    const { container } = mount({ id: 'e1', kind: 'message', text: '' });
    expect(container.querySelector('.wrap')).toBeNull();
  });
});
