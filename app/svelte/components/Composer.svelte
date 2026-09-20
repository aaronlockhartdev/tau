<script lang="ts">
  // Composer with the 3-way lane selector (spec §9): one control
  // picking force / steering / follow-up. Enter sends; shift+enter
  // breaks the line; force interrupts the in-flight turn, steering
  // delivers at the next tool-call opportunity, follow-up after the
  // model finishes.

  import { store, send, type PendingMsg } from '../lib/store.svelte';

  let text = $state('');
  let lane = $state<PendingMsg['lane']>('steering');

  const lanes: Array<{ id: PendingMsg['lane']; label: string; title: string }> = [
    { id: 'force', label: 'force', title: 'interrupt the in-flight turn now' },
    { id: 'steering', label: 'steering', title: 'deliver at the next tool-call opportunity' },
    { id: 'follow-up', label: 'follow-up', title: 'deliver after the model finishes its turn' }
  ];

  const running = $derived(
    store.current ? store.sessions[store.current].turn === 'running' : false
  );

  let inputEl = $state<HTMLTextAreaElement | null>(null);

  // The box grows with the text up to the cap, then scrolls.
  function fit(): void {
    if (!inputEl) return;
    inputEl.style.height = 'auto';
    inputEl.style.height = Math.min(inputEl.scrollHeight, 160) + 'px';
  }

  function submit(): void {
    const t = text.trim();
    if (!t) return;
    text = '';
    if (inputEl) inputEl.style.height = '';
    void send(t, lane);
  }
</script>

<div class="composer">
  <textarea
    class="input"
    bind:this={inputEl}
    bind:value={text}
    rows="3"
    placeholder="Message Tau — enter sends, shift+enter for a new line"
    onkeydown={(e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        submit();
      }
    }}
    oninput={fit}
  ></textarea>
  <div class="row">
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
    <button class="send" class:disabled={!text.trim()} onclick={submit}>
      {running && lane === 'force' ? '⚡' : '↑'}
    </button>
  </div>
</div>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 16px;
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
    width: 100%;
    min-height: 66px;
    max-height: 160px;
    resize: none;
    overflow-y: auto;
    background: #0c0e12;
    color: var(--tx);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 10px 12px;
    font: 13.5px/1.45 var(--sans, system-ui);
    outline: none;
  }
  .input:focus {
    border-color: rgba(76, 194, 255, 0.4);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .send {
    margin-left: auto;
    width: 32px;
    height: 32px;
    border-radius: 6px;
    background: var(--acc);
    color: #081018;
    font-size: 15px;
    font-weight: 700;
    flex: none;
  }
  .send.disabled {
    opacity: 0.35;
  }
</style>
