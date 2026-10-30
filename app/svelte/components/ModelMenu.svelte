<script lang="ts">
  // The model menu (the model's new home): floating and CENTERED in the
  // center column (the v4/v5 answer — one convention for all floating
  // menus), models grouped by provider (the workspace's provider rows), the
  // current model dot-highlighted. The composer's model chip and the
  // /model command both open it.
  //
  // The rows are the store's (#337), not a local first-open cache: the core
  // reads its config layers on every `provider_list` and emits
  // `provider_changed` when the rows it serves differ, so a provider added
  // to config.toml reaches a menu that is already open.
  import { onMount } from 'svelte';
  import type { ProviderInfo } from '../lib/wire/protocol';
  import { store, setModel, currentSession, fetchProviders } from '../lib/store/store.svelte';

  let error = $state<string | null>(null);

  const cur = $derived(currentSession());
  const model = $derived(cur?.meta.model ?? '');
  const ws = $derived(cur?.meta.workspace ?? null);
  const providers = $derived<ProviderInfo[] | null>(ws ? (store.providers[ws] ?? null) : null);

  function close(): void {
    store.modelMenuOpen = false;
  }

  function select(qualified: string): void {
    void setModel(qualified);
    close();
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') close();
  }

  onMount(() => {
    window.addEventListener('keydown', onKey);
    // Opening is a fresh read: the core serves the rows from the current
    // files, so a menu never shows a provider the config no longer has.
    if (ws) {
      void fetchProviders(ws).catch((e: unknown) => {
        error = e instanceof Error ? e.message : String(e);
      });
    }
    return () => window.removeEventListener('keydown', onKey);
  });
</script>

<div
  class="scrim"
  role="presentation"
  tabindex="-1"
  onclick={close}
  onkeydown={(e) => {
    if (e.key === 'Escape' || e.key === 'Enter') close();
  }}
></div>
<div class="mmenu" role="menu">
  <div class="mh">model <span class="esc">esc to dismiss</span></div>
  {#if error}
    <div class="merr">{error}</div>
  {:else if providers === null}
    <div class="merr">loading…</div>
  {:else}
    {#each providers as p (p.name)}
      <div class="mgrp">
        <div class="gn">{p.name}</div>
        {#each p.models as m (m)}
          {@const q = `${p.name}/${m}`}
          {@const isCur = model === m || model === q}
          <div
            class="mrow"
            class:cur={isCur}
            role="menuitem"
            tabindex="-1"
            onkeydown={(e) => {
              if (e.key === 'Enter') select(q);
            }}
            onclick={() => select(q)}
          >
            <span class="dot"></span>
            <span class="mn">{m}</span>
            {#if isCur}<span class="mk">current</span>{/if}
          </div>
        {/each}
      </div>
    {/each}
  {/if}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
    background: rgba(0, 0, 0, 0.35);
  }
  .mmenu {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 40;
    width: 340px;
    overflow: hidden;
    background: var(--panel2);
    border: 1px solid var(--line2);
    border-radius: 12px;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.55);
  }
  .mh {
    padding: 9px 14px 7px;
    font: 10px var(--mono);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--faint);
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .mh .esc {
    margin-left: auto;
    letter-spacing: 0;
  }
  .merr {
    padding: 12px 14px;
    font: 11px var(--mono);
    color: var(--dim);
  }
  .mgrp {
    padding: 4px 0 2px;
  }
  .gn {
    padding: 3px 14px 2px;
    font: 10px var(--mono);
    color: var(--faint);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .mrow {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 5px 14px;
    font: 12px var(--mono);
    color: var(--tx);
    cursor: pointer;
  }
  .mrow:hover {
    background: color-mix(in srgb, var(--acc) 10%, transparent);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: transparent;
    flex: none;
  }
  .mrow.cur .dot {
    background: var(--acc);
  }
  .mrow.cur .mn {
    color: var(--acc);
  }
  .mk {
    margin-left: auto;
    color: var(--faint);
    font-size: 10px;
  }
</style>
