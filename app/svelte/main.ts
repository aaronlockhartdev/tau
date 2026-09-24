// The WebdriverIO frontend plugin (roadmap G2): auto-initialises the
// window.wdioTauri seam before the app's module graph runs.
import '@wdio/tauri-plugin';
import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { init, applyEvents } from './lib/store.svelte';
import { onEvents, isTauri } from './lib/protocol';
// Connect at module scope (once, before mount) — not in a component
// effect: init mutates the store, and a write during an effect's run
// re-triggers that effect in this Svelte (update-depth loop).
void init();
if (isTauri()) {
  // Dev seam: a rolling log of the event types the wire delivers
  // (window.__evlog) — the diagnosis aid for stream/event bugs.
  void onEvents((evs) => {
    const w = window as unknown as { __evlog?: string[] };
    w.__evlog = w.__evlog ?? [];
    for (const e of evs) {
      // Stream events log their call id's tail; om_status logs its kind —
      // enough to trace a turn's stream sequence without payload noise.
      const extra =
        e.type === 'om_status'
          ? `:${e.kind}`
          : e.type === 'stream_start' || e.type === 'stream_end'
            ? `:${e.call_id.slice(-4)}${e.type === 'stream_end' && e.interrupted ? '!i' : ''}`
            : '';
      w.__evlog.push(e.type + extra);
      if (w.__evlog.length > 200) w.__evlog.shift();
    }
    applyEvents(evs);
  });
}

const root = document.getElementById('app');
if (!root) throw new Error('missing #app element');
const app = mount(App, { target: root });

export default app;
