// Vitest covers the lib + component suites; the WDIO specs under tests/e2e
// (roadmap G2) run under the WebdriverIO testrunner (wdio.conf.mjs), not vitest.
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [svelte(), svelteTesting()],
  test: {
    environment: 'jsdom',
    // The shiki highlighter loads all 346 bundled grammars (~1.4 s); under CI
    // + coverage instrumentation that can exceed the 5 s default, so give the
    // highlight / warmup-dependent tests headroom.
    testTimeout: 20000,
    setupFiles: ['svelte/vitest-setup.ts'],
    exclude: ['**/node_modules/**', '**/dist/**', 'tests/e2e/**'],
    coverage: {
      provider: 'v8',
      include: ['svelte/**/*.{ts,svelte}'],
      exclude: ['**/*.{test,spec}.*', 'svelte/lib/testing/**'],
      reporter: ['text', 'lcov', 'html']
    }
  }
});
