<script lang="ts">
  // Layout skeleton only (spec §9): workspace tabs on top, chat central.
  // Full GUI behavior lands with tickets #25/#26.
  import { onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';

  let workspace = 'workspace';
  let transcriptEl: HTMLDivElement;

  // The one wired live path (ticket #21): coalesced event batches from the
  // core — main.rs emits them on the `tau://event` channel — reach the
  // shell. Each event is a tagged-union JSON object; the skeleton appends
  // a line per event type.
  const unlisten = listen<Record<string, unknown>[]>('tau://event', (event) => {
    for (const item of event.payload) {
      const type = (item.type as string | undefined) ?? 'event';
      transcriptEl.insertAdjacentHTML('beforeend', `<div class="event">${type}</div>`);
    }
  });
  onDestroy(() => {
    unlisten.then((fn) => fn());
  });
</script>

<div class="shell">
  <header class="tab-bar">
    <div class="tab active" aria-current="page">{workspace}</div>
  </header>
  <main class="transcript" bind:this={transcriptEl} aria-label="transcript"></main>
</div>

<style>
  .shell {
    display: grid;
    grid-template-rows: auto 1fr;
    height: 100vh;
  }

  .tab-bar {
    display: flex;
    gap: 4px;
    padding: 4px 8px 0;
    border-bottom: 1px solid #333;
  }

  .tab {
    padding: 6px 14px;
    border-radius: 6px 6px 0 0;
  }

  .tab.active {
    background: rgba(255, 255, 255, 0.08);
  }

  .transcript {
    overflow-y: auto;
  }

  .event {
    padding: 2px 8px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
    opacity: 0.8;
  }

  :global(html),
  :global(body) {
    margin: 0;
    background: #1e1e1e;
    color: #ddd;
    font-family: system-ui, sans-serif;
  }
</style>
