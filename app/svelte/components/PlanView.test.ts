// @vitest-environment jsdom
// The plan pane's view: steps with done/open state, the N/M progress, the
// current step (first open item), the gate badge, and the empty state.
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import type { Plan } from '../lib/wire/protocol';
import PlanView from './PlanView.svelte';

const plan = (over: Partial<Plan> = {}): Plan => ({
  steps: [],
  criteria: '',
  notes: '',
  ...over
});

const STEPS = plan({
  steps: [
    { text: 'investigate the failure', done: true },
    { text: 'fix the parser', done: false },
    { text: 'add tests', done: false }
  ],
  criteria: 'build passes'
});

describe('PlanView', () => {
  it('renders the steps, N/M, the current step, and the gate badge', () => {
    render(PlanView, { props: { plan: STEPS, gateFailed: false } });
    expect(screen.getByText('1/3')).toBeInTheDocument();
    expect(screen.getByText('investigate the failure')).toBeInTheDocument();
    expect(screen.getByText('fix the parser')).toBeInTheDocument();
    expect(screen.getByText('add tests')).toBeInTheDocument();
    expect(screen.getByText('gate ok')).toBeInTheDocument();
    // The first open item is the current step (highlighted).
    expect(screen.getByText('fix the parser').closest('.step')).toHaveClass('current');
    expect(screen.getByText('investigate the failure').closest('.step')).toHaveClass('done');
  });

  it('shows the gate-failed badge when the gate is failed', () => {
    render(PlanView, { props: { plan: STEPS, gateFailed: true } });
    expect(screen.getByText('gate failed')).toBeInTheDocument();
  });

  it('renders the criteria section when present', () => {
    render(PlanView, { props: { plan: STEPS, gateFailed: false } });
    expect(screen.getByText('build passes')).toBeInTheDocument();
  });

  it('renders the notes section when present', () => {
    render(PlanView, { props: { plan: plan({ steps: STEPS.steps, notes: 'still debugging' }), gateFailed: false } });
    expect(screen.getByText('still debugging')).toBeInTheDocument();
  });

  it('shows the empty state for a plan with no steps', () => {
    render(PlanView, { props: { plan: plan(), gateFailed: false } });
    expect(screen.getByText('No plan yet — the agent writes it as it works')).toBeInTheDocument();
    expect(screen.queryByText('0/0')).toBeNull();
  });
});
