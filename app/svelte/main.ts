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
  void onEvents(applyEvents);
}

const root = document.getElementById('app');
if (!root) throw new Error('missing #app element');
const app = mount(App, { target: root });

export default app;
