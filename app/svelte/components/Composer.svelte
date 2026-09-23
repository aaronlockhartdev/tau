<script lang="ts">
  // The v5 composer: a rounded floating box (margin 0 16px 12px, line2
  // border, panel background). The textarea sits alone in the input row
  // (full box width — it must not share the row with the controls); the
  // footer row carries the meta items (model chip + hint) left and the
  // 3-way lane selector + send button right. Enter sends; shift+enter
  // breaks the line; force interrupts the in-flight turn, steering delivers
  // at the next tool-call opportunity, follow-up after the model finishes.
  // The lanes dim while the session is not running but stay changeable
  // (the selection applies to the next send); the send button becomes a
  // stop button while a turn is running and the field is empty.
  //
  // A leading `/` opens the command dropdown (ticket #28, extended with
  // /model and /help): arrows navigate, Enter completes, Tab completes,
  // Esc dismisses; completion is plain text until send, and the /skill:
  // expansion happens at the message_send boundary, not here.
  import { store, send, stop, type PendingMsg } from '../lib/store.svelte';
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

  // The dropdown's rows: the fixed commands (/model, /help) and the
  // matching skills — one list in every state.
  type Row = { id: string; name: string; desc: string; skill?: SkillInfo };
  const matches = $derived.by<Row[]>(() => {
    if (prefix === null || completed) return [];
    const out: Row[] = [];
    if ('model'.startsWith(prefix)) {
      out.push({ id: 'model', name: '/model', desc: "switch the session's model" });
    }
    if ('help'.startsWith(prefix)) {
      out.push({ id: 'help', name: '/help', desc: 'list commands' });
    }
    for (const s of skills
      .filter((s) => s.name.startsWith(prefix))
      .sort((a, b) => a.name.localeCompare(b.name))) {
      out.push({ id: `skill:${s.name}`, name: `/skill:${s.name}`, desc: s.description, skill: s });
    }
    return out;
  });
  const open = $derived(matches.length > 0);
  let sel = $state(0);

  function fit(): void {
    if (!inputEl) return;
    inputEl.style.height = 'auto';
    inputEl.style.height = Math.min(inputEl.scrollHeight, 160) + 'px';
  }

  function complete(i: number): void {
    const m = matches[i];
    if (!m) return;
    if (m.id === 'model') {
      // /model opens the same centered model menu the chip does.
      text = '';
      completed = true;
      store.modelMenuOpen = true;
      requestAnimationFrame(() => inputEl?.focus());
      return;
    }
    if (m.id === 'help') {
      // "list commands": a fresh / shows the whole command list.
      text = '/';
      completed = false;
      sel = 0;
      requestAnimationFrame(() => {
        inputEl?.focus();
        inputEl?.setSelectionRange(text.length, text.length);
      });
      fit();
      return;
    }
    const s = m.skill!;
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
    store.modelMenuOpen = false;
    if (inputEl) inputEl.style.height = '';
    void send(t, lane);
  }

  function onKey(e: KeyboardEvent): void {
    if (store.modelMenuOpen && e.key === 'Escape') {
      e.preventDefault();
      store.modelMenuOpen = false;
      return;
    }
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
      {#each matches as m, i (m.id + m.name)}
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
          <span class="oname">{m.name}</span>
          <span class="odesc">{m.desc}</span>
        </div>
      {/each}
    </div>
  {/if}
  <div class="crow">
    <textarea
      class="cin"
      bind:this={inputEl}
      bind:value={text}
      rows="1"
      placeholder="message — / for commands"
      onkeydown={onKey}
      onblur={() => (completed = true)}
      oninput={() => {
        completed = false;
        sel = 0;
        fit();
      }}
    ></textarea>
  </div>
  <div class="cfoot">
    <div class="cmeta">
      <button
        class="mchip"
        title="switch model"
        onclick={() => (store.modelMenuOpen = !store.modelMenuOpen)}
      >
        <svg class="mi" viewBox="0 0 24 24" aria-hidden="true"><use href="#i-bot" /></svg>
        <span class="mname">{cur?.meta.model ? cur.meta.model.split('/').pop() : 'no model'}</span>
        <svg class="chev" viewBox="0 0 24 24" aria-hidden="true"><use href="#i-chev" /></svg>
      </button>
      <span class="chint">enter send · shift+enter newline · / commands</span>
    </div>
    <div class="lanes" class:dim={!running}>
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
    <button
      class="send"
      class:stop={running && !text.trim()}
      class:disabled={!text.trim() && !running}
      title={running && !text.trim() ? 'stop the in-flight turn' : 'send'}
      onclick={running && !text.trim() ? () => void stop() : submit}
    >
      {running ? (text.trim() ? (lane === 'force' ? '⚡' : '↑') : '■') : '↑'}
    </button>
  </div>
</div>

<style>
  .composer {
    position: relative;
    flex: none;
    margin: 0 16px 12px;
    border: 1px solid var(--line2);
    border-radius: 10px;
    background: var(--panel);
  }
  .crow {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
  }
  .cin {
    flex: 1;
    background: none;
    border: none;
    outline: none;
    resize: none;
    font: 13px/1.4 var(--sans);
    color: var(--tx);
    min-height: 18px;
    max-height: 160px;
  }
  .cin::placeholder {
    color: var(--faint);
  }
  .lanes {
    display: flex;
    border: 1px solid var(--line);
    border-radius: 8px;
    overflow: hidden;
    flex: none;
  }
  .lanes.dim {
    opacity: 0.4;
  }
  .lane {
    padding: 4px 10px;
    font: 11px var(--mono);
    color: var(--dim);
    border-right: 1px solid var(--line);
    background: none;
    cursor: pointer;
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
  .send {
    width: 26px;
    height: 26px;
    border-radius: 7px;
    background: var(--acc);
    color: #060a10;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    flex: none;
    border: none;
    cursor: pointer;
  }
  .send.disabled {
    opacity: 0.35;
  }
  .send.stop {
    background: var(--red);
    color: #100606;
  }
  .cfoot {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 12px 9px;
  }
  .cmeta {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-right: auto;
  }
  /* The model chip is minimal: no border, no fill — dim text, a faint
     icon, a small chevron; hover lifts the text to primary. */
  .mchip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: 10.5px var(--mono);
    color: var(--faint);
    padding: 3px 2px;
    cursor: pointer;
    background: none;
    border: none;
  }
  .mchip:hover {
    color: var(--dim);
  }
  .mchip .mi {
    width: 11px;
    height: 11px;
    stroke: var(--faint);
    fill: none;
    stroke-width: 2;
  }
  .mchip .chev {
    width: 8px;
    height: 8px;
    stroke: var(--faint);
    fill: none;
    stroke-width: 2;
  }
  .mchip .mname {
    color: var(--dim);
  }
  .mchip:hover .mname {
    color: var(--tx);
  }
  .chint {
    font: 10px var(--mono);
    color: var(--faint);
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
