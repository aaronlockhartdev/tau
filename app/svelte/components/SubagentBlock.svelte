<script lang="ts">
  // The crew row inline in the parent transcript (P2): the live state of one
  // sub-agent at its spawn point. The state is the block's edge; the header
  // opens the child session; `follow` pins the follow filter to this worker.

  import { fmtK } from '../lib/display/format';
  import type { CrewView } from '../lib/display/subagent-view';

  let {
    view,
    name,
    followed = false,
    onFollow,
    onOpen
  }: {
    view: CrewView;
    name: string;
    followed?: boolean;
    onFollow: () => void;
    onOpen: () => void;
  } = $props();

  const label = $derived(
    view.phase === 'running'
      ? 'running'
      : view.phase === 'waiting-user'
        ? 'waiting on you'
        : view.phase === 'waiting-parent'
          ? 'waiting on parent'
          : view.phase === 'done'
            ? 'done'
            : view.phase === 'failed'
              ? 'failed'
              : view.phase === 'stopped'
                ? 'stopped'
                : 'spawned'
  );

  const tokens = $derived(
    view.inputTokens !== null && view.outputTokens !== null
      ? `${fmtK(view.inputTokens)} in · ${fmtK(view.outputTokens)} out`
      : ''
  );
</script>

<div class="subblock p-{view.phase}">
  <button class="head" type="button" disabled={!view.child} onclick={onOpen}>
    <span class="dot"></span>
    <span class="name">{name}</span>
    <span class="chip">{view.agentType}</span>
    <span class="chip dim">{view.contextMode}</span>
    <span class="state">{label}</span>
    {#if tokens}<span class="meta">{tokens}</span>{/if}
    {#if view.child}<span class="open">open ↗</span>{/if}
  </button>
  {#if view.lastMessage}
    <div class="line" class:report={view.phase === 'done'}>{view.lastMessage}</div>
  {/if}
  {#if view.handle}
    <button
      class="follow"
      type="button"
      class:on={followed}
      onclick={onFollow}
    >{followed ? 'following' : 'follow'}</button>
  {/if}
</div>

<style>
  .subblock {
    margin: 0 16px 10px;
    padding: 8px 10px 8px 12px;
    border: 1px solid var(--line);
    border-left-width: 3px;
    border-radius: 8px;
    background: var(--panel2);
  }
  .subblock.p-running {
    border-left-color: var(--acc);
  }
  .subblock.p-waiting-user {
    border-left-color: var(--red);
  }
  .subblock.p-waiting-parent {
    border-left-color: var(--amber);
  }
  .subblock.p-done {
    border-left-color: var(--green);
  }
  .subblock.p-failed {
    border-left-color: var(--red);
  }
  .subblock.p-stopped,
  .subblock.p-spawn {
    border-left-color: var(--line2);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .head:disabled {
    cursor: default;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--dim);
    flex: none;
  }
  .p-running .dot {
    background: var(--acc);
    animation: pulse 1.4s infinite;
  }
  .p-waiting-user .dot {
    background: var(--red);
    animation: pulse 1.4s infinite;
  }
  .p-waiting-parent .dot {
    background: var(--amber);
  }
  .p-done .dot {
    background: var(--green);
  }
  .p-failed .dot {
    background: var(--red);
  }
  .name {
    font-weight: 600;
    font-size: 12.5px;
  }
  .chip {
    font: 10px var(--mono);
    color: var(--dim);
    background: var(--panel3);
    border-radius: 4px;
    padding: 1px 5px;
  }
  .chip.dim {
    color: var(--faint);
  }
  .state {
    margin-left: auto;
    font: 10px var(--mono);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--faint);
  }
  .p-running .state {
    color: var(--acc);
  }
  .p-waiting-user .state {
    color: var(--red);
  }
  .p-done .state {
    color: var(--green);
  }
  .p-failed .state {
    color: var(--red);
  }
  .meta {
    font: 10px var(--mono);
    color: var(--faint);
  }
  .open {
    font: 10px var(--mono);
    color: var(--acc);
  }
  .line {
    margin: 6px 0 0 14px;
    font-size: 12px;
    color: var(--dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .line.report {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--tx);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 6px 8px;
    margin-top: 8px;
  }
  .follow {
    margin: 6px 0 0 14px;
    background: none;
    border: 1px solid var(--line2);
    border-radius: 6px;
    padding: 2px 8px;
    font: 10.5px var(--mono);
    color: var(--dim);
    cursor: pointer;
  }
  .follow.on {
    color: var(--acc);
    border-color: var(--acc);
  }
</style>
