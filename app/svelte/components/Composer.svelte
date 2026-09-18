<script lang="ts">
  // Composer with the 3-way lane selector (spec §9): one control
  // picking force / steering / follow-up. Enter sends; force interrupts
  // the in-flight turn, steering delivers at the next tool-call
  // opportunity, follow-up after the model finishes.

  import { store, send, type PendingMsg } from '../lib/store.svelte';

  let text = $state('');
  let lane = $state<PendingMsg['lane']>('follow-up');

  const lanes: Array<{ id: PendingMsg['lane']; label: string; title: string }> = [
    { id: 'force', label: 'force', title: 'interrupt the in-flight turn now' },
    { id: 'steering', label: 'steering', title: 'deliver at the next tool-call opportunity' },
    { id: 'follow-up', label: 'follow-up', title: 'deliver after the model finishes its turn' }
  ];

  const running = $derived(
    store.current ? store.sessions[store.current].turn === 'running' : false
  );

  function submit(): void {
    const t = text.trim();
    if (!t) return;
    text = '';
    void send(t, lane);
  }
</script>

<div class="composer">
  <div class="lanes">
    {#each lanes as l (l.id)}
      <button
        class="lane"
        class:active={lane === l.id}
        title={l.title}
        onclick={() => (lane = l.id)}
      >
        {l.label}
      </button>
    {/each}
  </div>
  <input
    class="input"
    bind:value={text}
    placeholder="message — enter sends"
    onkeydown={(e) => e.key === 'Enter' && submit()}
  />
  <button class="send" class:disabled={!text.trim()} onclick={submit}>
    {running && lane === 'force' ? '⚡' : '↑'}
  </button>
</div>

<style>
  .composer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--line);
    background: var(--panel);
    flex: none;
  }
  .lanes {
    display: flex;
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
    flex: none;
  }
  .lane {
    padding: 4px 10px;
    font: 11px var(--mono);
    color: var(--dim);
    border-right: 1px solid var(--line);
  }
  .lane:last-child {
    border-right: none;
  }
  .lane:hover {
    color: var(--tx);
  }
  .lane.active {
    background: rgba(76, 194, 255, 0.12);
    color: var(--acc);
  }
  .input {
    flex: 1;
    background: #0c0e12;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 13.5px;
    outline: none;
  }
  .input:focus {
    border-color: rgba(76, 194, 255, 0.4);
  }
  .send {
    width: 34px;
    height: 34px;
    border-radius: 6px;
    background: var(--acc);
    color: #081018;
    font-size: 16px;
    font-weight: 700;
    flex: none;
  }
  .send.disabled {
    opacity: 0.35;
  }
</style>
