// The plan pane's pure projections over a parsed plan (spec §5.3): the
// N/M progress, the "current step" (the design: the first open item), and
// the fleet projection's gate verdict for one child.
import type { Plan } from '../wire/protocol';

export interface PlanProgress {
  // The checked-off step count.
  done: number;
  // The total step count.
  total: number;
  // The current step's text (the first open item); null when the plan is
  // empty or complete.
  current: string | null;
}

export function planProgress(plan: Plan): PlanProgress {
  const done = plan.steps.filter((s) => s.done).length;
  const current = plan.steps.find((s) => !s.done)?.text ?? null;
  return { done, total: plan.steps.length, current };
}

// The fleet projection's gate verdict for a child (design: the observed
// lifecycle settles it): `done` = plan complete and gate passed,
// `failed` = gate failed after retries; a live child's gate is pending.
export function fleetGate(state: string): 'pass' | 'fail' | 'pending' {
  if (state === 'done') return 'pass';
  if (state === 'failed') return 'fail';
  return 'pending';
}
