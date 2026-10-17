<script lang="ts">
  // The plan pane's view (spec §5.3): the session's parsed plan file —
  // steps with their done/open state, the N/M progress, the current step
  // (the first open item), and the gate status. A session with no plan
  // file (an empty plan) renders the empty state.
  import type { Plan } from '../lib/protocol';
  import { planProgress } from '../lib/plan';

  interface Props {
    plan: Plan;
    // The session's gate state (spec §5.3): the badge.
    gateFailed: boolean;
  }
  let { plan, gateFailed }: Props = $props();

  const progress = $derived(planProgress(plan));
  const criteria = $derived(plan.criteria.trim());
</script>

{#if progress.total === 0}
  <div class="emptyc">No plan yet — the agent writes it as it works</div>
{:else}
  <div class="head">
    <span class="prog">{progress.done}/{progress.total}</span>
    <span class="gate" class:fail={gateFailed}>{gateFailed ? 'gate failed' : 'gate ok'}</span>
  </div>
  <div class="steps">
    {#each plan.steps as s, i (i)}
      <div class="step" class:done={s.done} class:current={s.text === progress.current}>
        <span class="mark">{s.done ? '✓' : '○'}</span>
        <span class="txt">{s.text}</span>
      </div>
    {/each}
  </div>
  {#if criteria}
    <div class="ghead">criteria</div>
    <div class="crit">{criteria}</div>
  {/if}
{/if}

<style>
  .emptyc {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    font: 11px var(--mono);
    color: var(--faint);
    text-align: center;
    padding: 0 16px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px 6px;
  }
  .prog {
    font: 12px var(--mono);
    font-weight: 700;
    color: var(--tx);
  }
  .gate {
    margin-left: auto;
    font: 10px var(--mono);
    padding: 1px 7px;
    border-radius: 9px;
    border: 1px solid color-mix(in srgb, var(--green) 40%, transparent);
    color: var(--green);
  }
  .gate.fail {
    border-color: color-mix(in srgb, var(--red) 40%, transparent);
    color: var(--red);
  }
  .steps {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 2px 8px;
  }
  .step {
    display: flex;
    align-items: baseline;
    gap: 7px;
    padding: 3px 6px;
    border-radius: 6px;
    font-size: 12px;
    color: var(--tx);
  }
  .step .mark {
    flex: none;
    font: 11px var(--mono);
    color: var(--faint);
  }
  .step.done {
    color: var(--faint);
    text-decoration: line-through;
  }
  .step.done .mark {
    color: var(--green);
  }
  .step.current {
    background: var(--raised);
    color: var(--tx);
  }
  .step.current .mark {
    color: var(--acc);
  }
  .ghead {
    padding: 10px 12px 4px;
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--faint);
  }
  .crit {
    padding: 0 12px 12px;
    font-size: 12px;
    color: var(--dim);
    white-space: pre-wrap;
  }
</style>
