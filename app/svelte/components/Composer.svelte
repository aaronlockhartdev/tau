<script lang="ts">
  // Composer with the 3-way lane selector (spec §9): one control
  // picking force / steering / follow-up. Enter sends; shift+enter
  // breaks the line; force interrupts the in-flight turn, steering
  // delivers at the next tool-call opportunity, follow-up after the
  // model finishes.
  //
  // A leading `/` opens the /skill: autocomplete (ticket #28): arrows
  // navigate, Enter completes while the dropdown is open (sends while
  // closed), Tab completes, Esc or losing focus dismisses, and the mouse
  // completion is plain text until send; the expansion happens at the
  // message_send boundary, not here.
  import { store, send, type PendingMsg } from '../lib/store.svelte';
  import type { SkillInfo } from '../lib/protocol';

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

  // The dropdown's data source: the current session's workspace skill
  // registry (a disable-model-invocation skill is here too — the
  // dropdown is its only door).
  const cur = $derived(store.current ? store.sessions[store.current] : null);
  const skills: SkillInfo[] = $derived.by(() => {
    const ws = cur?.meta.workspace;
    return ws ? (store.skills[ws] ?? []) : [];
  });

  // The name prefix being typed after the leading `/` (the `skill:`
  // infix counts as already typed).
  const prefix = $derived.by(() => {
    if (!text.startsWith('/')) return null;
    const rest = text.slice(1);
    return rest.startsWith('skill:') ? rest.slice(6) : rest;
  });

  // A completed suggestion (Enter/Tab/click) keeps the dropdown closed
  // until the next keystroke; matches is empty in the meantime, so the
  // text stays plain until send.
  let completed = $state(false);

  const matches = $derived.by(() => {
    if (prefix === null || completed) return [];
    return skills
      .filter((s) => s.name.startsWith(prefix))
      .sort((a, b) => a.name.localeCompare(b.name));
  });
  const open = $derived(matches.length > 0);
  let sel = $state(0);

  function fit(): void {
    if (!inputEl) return;
    inputEl.style.height = 'auto';
    inputEl.style.height = Math.min(inputEl.scrollHeight, 160) + 'px';
  }

  function complete(i: number): void {
    const s = matches[i];
    if (!s) return;
    text = `/skill:${s.name} `;
    completed = true;
    sel = 0;
    requestAnimationFrame(() => {
      inputEl?.focus();
      inputEl?.setSelectionRange(text.length, text.length);
    });
    fit();
  }

  function submit(): void {
    const t = text.trim();
    if (!t) return;
    text = '';
    completed = false;
    if (inputEl) inputEl.style.height = '';
    void send(t, lane);
  }

  function onKey(e: KeyboardEvent): void {
    if (open) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        sel = (sel + 1) % matches.length;
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        sel = (sel - 1 + matches.length) % matches.length;
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        complete(sel);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        completed = true;
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        complete(sel);
        return;
      }
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }
</script>

<div class="composer">
  {#if open}
    <div class="dropdown" role="listbox">
      {#each matches as s, i (s.name)}
        <div
          class="opt"
          class:sel={i === sel}
          role="option"
          aria-selected={i === sel}
          tabindex="-1"
          onmouseenter={() => (sel = i)}
          onmousedown={(e) => e.preventDefault()}
          onkeydown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              complete(i);
            }
          }}
          onclick={() => complete(i)}
        >
          <span class="oname">/{s.name}</span>
          <span class="odesc">{s.description}</span>
        </div>
      {/each}
    </div>
  {/if}
  <textarea
    class="input"
    bind:this={inputEl}
    bind:value={text}
    rows="3"
    placeholder="Message Tau — enter sends, shift+enter for a new line"
    onkeydown={onKey}
    onblur={() => (completed = true)}
    oninput={() => {
      completed = false;
      sel = 0;
      fit();
    }}
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
    position: relative;
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
    border-radius: 8px;
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
    background: color-mix(in srgb, var(--acc) 12%, transparent);
    color: var(--acc);
  }
  .input {
    width: 100%;
    min-height: 66px;
    max-height: 160px;
    resize: none;
    overflow-y: auto;
    background: var(--bg);
    color: var(--tx);
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 10px 12px;
    font: 13.5px/1.45 var(--sans, system-ui);
    outline: none;
  }
  .input:focus {
    border-color: color-mix(in srgb, var(--acc) 40%, transparent);
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
    color: var(--bg);
    font-size: 15px;
    font-weight: 700;
    flex: none;
  }
  .send.disabled {
    opacity: 0.35;
  }
  .dropdown {
    position: absolute;
    bottom: 100%;
    left: 16px;
    right: 16px;
    margin-bottom: -1px;
    background: var(--panel2);
    border: 1px solid var(--line);
    border-radius: 8px 8px 0 0;
    max-height: 220px;
    overflow-y: auto;
    z-index: 10;
  }
  .opt {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 7px 12px;
    cursor: pointer;
  }
  .opt.sel {
    background: color-mix(in srgb, var(--acc) 12%, transparent);
  }
  .oname {
    font: 12px var(--mono);
    color: var(--acc);
    flex: none;
  }
  .odesc {
    font: 12px var(--sans, system-ui);
    color: var(--dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
