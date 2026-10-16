import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    strictPort: true
  },
  build: {
    outDir: 'svelte/dist',
    // Each shiki grammar is its own chunk and we now use all of them; a few
    // (C++, Emacs Lisp) are inherently >500kB, so raise the limit instead of
    // fighting an expected warning.
    chunkSizeWarningLimit: 1000,
    rollupOptions: {
      input: ['./index.html']
    }
  },
  test: {
    // The shiki highlighter loads all 346 bundled grammars (~1.4 s); under CI
    // + coverage instrumentation that can exceed the 5 s default, so give the
    // highlight / warmup-dependent tests headroom.
    testTimeout: 20000
  },
}
);

