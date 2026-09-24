<script lang="ts">
  // One unsplit bar: a left context group (state · workspace · session
  // name · the om gauge) and a right cost group (in · out · cache, and
  // tps only while streaming).
  import { store } from '../lib/store.svelte';

  const s = $derived(store.current ? store.sessions[store.current] : null);
  const running = $derived(s?.turn === 'running');
  const name = $derived(s?.meta.title ?? s?.meta.id ?? '—');
  const ws = $derived(
    s
      ? (store.workspaces.find((w) => w.id === s.meta.workspace)?.name ?? s.meta.workspace)
      : '—'
  );
  const cachePct = $derived(
    s?.usage && s.usage.input_tokens > 0
      ? Math.round((100 * s.usage.cached_prompt_tokens) / s.usage.input_tokens)
      : 0
  );
  function fmt(n: number): string {
    return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`;
  }
</script>

<div class="bar">
  <span class="bl">
    <span class="sdot" class:run={running}></span>
    <span class="stt" class:dim={!running}>{running ? 'running' : 'idle'}</span>
    <span class="sep">·</span>
    <span>{ws}</span>
    <span class="sep">·</span>
    <span>{name}</span>
    <span class="sep">·</span>
    <span class="k">om</span>
    {#if s && s.om.kind === 'idle'}
      <span class="om">{fmt(s.om.observation_tokens)}/{fmt(s.om.reflector_threshold)}</span>
    {:else if s}
      <span class="om busy">{s.om.kind}<span class="pulse">…</span></span>
    {/if}
  </span>
  <span class="br">
    {#if s?.usage}
      <span>{fmt(s.usage.input_tokens)} in</span>
      <span class="sep">·</span>
      <span>{fmt(s.usage.output_tokens)} out</span>
      <span class="sep">·</span>
      <span>{cachePct}% cache</span>
    {:else}
      <span>0 in</span>
      <span class="sep">·</span>
      <span>0 out</span>
      <span class="sep">·</span>
      <span>0% cache</span>
    {/if}
    {#if running}
      <span class="sep">·</span>
      <span class="tpsv">{Math.round(s?.tps ?? 0)} tps</span>
    {/if}
  </span>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 26px;
    flex: none;
    padding: 0 12px;
    background: var(--bg);
    border-top: 1px solid var(--line);
    font: 11px var(--mono);
    color: var(--dim);
    user-select: none;
    overflow: hidden;
    gap: 12px;
  }
  .bl,
  .br {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    white-space: nowrap;
    /* flex items refuse to shrink below content without this; the bar must
       never push the app wider than the window */
    min-width: 0;
    overflow: hidden;
  }
  .k {
    color: var(--faint);
    text-transform: uppercase;
    font-size: 9.5px;
  }
  .sdot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--faint);
    flex: none;
  }
  .sdot.run {
    background: var(--green);
    box-shadow: 0 0 6px color-mix(in srgb, var(--green) 60%, transparent);
  }
  .stt {
    color: var(--tx);
    font-weight: 500;
  }
  .stt.dim {
    color: var(--dim);
    font-weight: 400;
  }
  .om {
    color: var(--faint);
  }
  .om.busy {
    color: var(--acc);
  }
  .om .pulse {
    display: inline-block;
    animation: pl 1.2s ease-in-out infinite;
  }
  @keyframes pl {
    50% {
      opacity: 0.35;
    }
  }
  .sep {
    color: var(--faint);
  }
  .tpsv {
    color: var(--tx);
  }
</style>
