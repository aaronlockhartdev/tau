// Vitest covers the store suite only; the WDIO specs under tests/e2e (roadmap
// G2) run under the WebdriverIO testrunner (wdio.conf.mjs), not vitest.
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [svelte()],
  test: {
    exclude: ['**/node_modules/**', '**/dist/**', 'tests/e2e/**']
  }
});
