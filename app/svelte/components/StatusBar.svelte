<script lang="ts">
  // Lualine-style 3-segment bottom bar (spec §9):
  //   [workspace · session · state]   [entries · rendered · fps]   [model · usage]

  import { store } from '../lib/store.svelte';

  const s = $derived(store.current ? store.sessions[store.current] : null);
  let fps = $state(0);

  let frames = 0;
  const timer = setInterval(() => {
    fps = frames;
    frames = 0;
  }, 1000);

  let raf = 0;
  function tick(): void {
    frames++;
    raf = requestAnimationFrame(tick);
  }
  tick();

  $effect(() => {
    return () => {
      clearInterval(timer);
      cancelAnimationFrame(raf);
    };
  });

  function fmt(n: number): string {
    return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
  }
</script>

<div class="bar">
  <span class="seg s1">
    <span class="k">ws</span> {s?.meta.workspace ?? '—'}
    <span class="k">ses</span> {s?.meta.id ?? '—'}
    <span class="k">st</span> {s?.turn === 'running' ? 'running' : 'idle'}
  </span>
  <span class="seg s2">
    <span class="k">entries</span> {s ? fmt(s.entries.length + s.live.length) : 0}
    <span class="k">live</span> {s?.live.length ?? 0}
    <span class="k">fps</span> {fps}
  </span>
  <span class="seg s3">
    <span class="k">model</span> {s?.meta.model ?? '—'}
    {#if s?.usage}
      <span class="k">tok</span> {fmt(s.usage.in + s.usage.out)}
      {#if s.usage.cost > 0}$<span>{s.usage.cost.toFixed(4)}</span>{/if}
    {/if}
  </span>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    height: 26px;
    background: #0b0c10;
    border-top: 1px solid var(--line);
    font: 11px var(--mono);
    color: var(--dim);
    user-select: none;
  }
  .seg {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 12px;
    height: 100%;
    white-space: nowrap;
    overflow: hidden;
  }
  .s1 {
    flex: 1.4;
  }
  .s2 {
    flex: 1;
    border-left: 1px solid var(--line);
  }
  .s3 {
    flex: 1.4;
    border-left: 1px solid var(--line);
    justify-content: flex-end;
  }
  .k {
    color: #454c59;
    text-transform: uppercase;
    font-size: 9.5px;
  }
</style>
