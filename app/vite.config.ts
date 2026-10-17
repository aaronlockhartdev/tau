import { defineConfig } from 'vite';
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
  }
}
);

