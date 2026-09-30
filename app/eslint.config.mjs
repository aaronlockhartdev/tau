// Flat ESLint config for the app (Svelte 5 + TS). Scope: svelte/** (.svelte +
// .ts) and the app-root config files; e2e specs, build output, and caches are
// ignored. Style rules carry the repo conventions (2-space, single quotes,
// semicolons on) — no separate formatter.
import js from '@eslint/js';
import { loadConfig } from '@sveltejs/load-config';
import stylistic from '@stylistic/eslint-plugin';
import globals from 'globals';
import sveltePlugin from 'eslint-plugin-svelte';
import svelteParser from 'svelte-eslint-parser';
import tseslint from 'typescript-eslint';

// No svelte.config.js exists — the Svelte config is inline in vite.config.ts,
// so load it from there (eslint-plugin-svelte README, "Svelte config inside
// vite.config.js").
const svelteConfig = (await loadConfig(process.cwd()))?.config;

const styleFiles = ['svelte/**/*.{ts,svelte}', 'vite.config.ts', 'vitest.config.mjs', 'wdio.conf.mjs'];

export default [
  {
    ignores: [
      '.svelte-check/**',
      'node_modules/**',
      'dist/**',
      'svelte/dist/**',
      'svelte/scripts/**',
      'tests/e2e/**',
      'coverage/**'
    ]
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...sveltePlugin.configs['flat/recommended'],
  {
    files: ['**/*.svelte', '**/*.svelte.js', '**/*.svelte.ts'],
    languageOptions: {
      globals: globals.browser,
      parser: svelteParser,
      parserOptions: {
        parser: tseslint.parser,
        extraFileExtensions: ['.svelte'],
        svelteConfig
      }
    },
    rules: {
      // Markdown is rendered through {@html} by design (Transcript/EntryCard).
      'svelte/no-at-html-tags': 'off',
      // The store keeps plain (non-reactive) Maps on purpose (store.svelte);
      // SvelteMap would change the reactivity semantics.
      'svelte/prefer-svelte-reactivity': 'off',
      // False positive on `export type { … } from '…'` re-exports in .svelte files.
      'no-import-assign': 'off'
    }
  },
  {
    files: ['**/*.mjs'],
    languageOptions: { globals: globals.node }
  },
  {
    // The virtua bind site needs `as any` (documented at the ref declaration);
    // svelte 5.57 predates svelte/typed's InstanceOf, so no cast-free form type-checks.
    files: ['svelte/components/Transcript.svelte'],
    rules: { '@typescript-eslint/no-explicit-any': 'off' }
  },
  {
    rules: {
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }]
    }
  },
  {
    files: styleFiles,
    plugins: { '@stylistic': stylistic },
    rules: {
      '@stylistic/indent': ['error', 2],
      '@stylistic/quotes': ['error', 'single'],
      '@stylistic/semi': ['error', 'always']
    }
  }
];
