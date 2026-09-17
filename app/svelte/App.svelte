<script lang="ts">
  // Layout skeleton only (spec §9): workspace tabs on top, chat central.
  // Behavior lands with the GUI milestones.
  let workspace = 'workspace';
</script>

<div class="shell">
  <header class="tab-bar">
    <div class="tab active" aria-current="page">{workspace}</div>
  </header>
  <main class="transcript" aria-label="transcript"></main>
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

  :global(html),
  :global(body) {
    margin: 0;
    background: #1e1e1e;
    color: #ddd;
    font-family: system-ui, sans-serif;
  }
</style>
