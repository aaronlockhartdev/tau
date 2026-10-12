// @vitest-environment jsdom
// The crew row (P2): its phase presentation — static spawn marker when cold,
// live state/tokens/line while running, the red waiting-on-you case, the
// done report — plus the follow and open interactions.

import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { SubagentInfo } from '../lib/protocol';
import { crewView, type SpawnRef } from '../lib/subagent-view';
import SubagentBlock from './SubagentBlock.svelte';

const ref: SpawnRef = {
  entryId: 't1',
  agentType: 'general',
  contextMode: 'fresh',
  brief: 'look at the config'
};

function info(over: Partial<SubagentInfo> = {}): SubagentInfo {
  return {
    handle: 'h1',
    child: 'child-1',
    agent_type: 'general',
    context_mode: 'fresh',
    state: 'running',
    waiting_on: null,
    last_message: null,
    usage: null,
    task: null,
    resume_contract: null,
    ...over
  };
}

function block(over: { followed?: boolean; view?: ReturnType<typeof crewView> } = {}) {
  const onFollow = vi.fn();
  const onOpen = vi.fn();
  render(SubagentBlock, {
    view: over.view ?? crewView(ref, info()),
    name: 'worker',
    followed: over.followed ?? false,
    onFollow,
    onOpen
  });
  return { onFollow, onOpen };
}

describe('SubagentBlock', () => {
  it('shows the static spawn marker when the crew is cold', () => {
    block({ view: crewView(ref, null) });
    expect(screen.getByText('spawned')).toBeTruthy();
    expect(screen.queryByText(/follow/)).toBeNull();
    expect(screen.queryByText(/open/)).toBeNull();
  });

  it('shows the live state, tokens and latest line while running', () => {
    block({
      view: crewView(
        ref,
        info({
          last_message: 'reading the config',
          usage: {
            input_tokens: 1200,
            output_tokens: 80,
            total_tokens: 1280,
            cached_prompt_tokens: 0
          }
        })
      )
    });
    expect(screen.getByText('running')).toBeTruthy();
    expect(screen.getByText('1.2k in · 80 out')).toBeTruthy();
    expect(screen.getByText('reading the config')).toBeTruthy();
  });

  it('marks the waiting-on-you case', () => {
    block({ view: crewView(ref, info({ state: 'idle', waiting_on: 'user' })) });
    expect(screen.getByText('waiting on you')).toBeTruthy();
  });

  it('renders the report for a done child', () => {
    block({ view: crewView(ref, info({ state: 'done', last_message: 'report: all green' })) });
    expect(screen.getByText('done')).toBeTruthy();
    expect(screen.getByText('report: all green')).toBeTruthy();
  });

  it('the follow button reports a toggle', async () => {
    const { onFollow } = block();
    await userEvent.click(screen.getByRole('button', { name: 'follow' }));
    expect(onFollow).toHaveBeenCalledOnce();
  });

  it('the header opens the child', async () => {
    const { onOpen } = block();
    await userEvent.click(screen.getByText('open ↗'));
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it('the header is inert when there is no child (cold)', () => {
    block({ view: crewView(ref, null) });
    const head = screen.getByRole('button', { name: /worker/ });
    expect(head).toBeDisabled();
  });
});
