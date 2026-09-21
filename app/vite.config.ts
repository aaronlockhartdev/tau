import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig(({ mode }) => ({
  plugins: [svelte()],
  server: {
    port: 5173,
    strictPort: true
  },
  build: {
    outDir: 'svelte/dist',
    // The demo entry is a dev-only rig fixture (ticket #29): the 'demo'
    // mode build ships it for vite preview / the verification script; the
    // release build bundles index.html only — the Tauri product loads that
    // one.
    rollupOptions: {
      input: mode === 'demo' ? ['./index.html', './demo.html'] : ['./index.html']
    }
  }
}));
