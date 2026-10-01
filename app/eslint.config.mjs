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
      '**/.svelte-check/**',
      'node_modules/**',
      'dist/**',
      'svelte/dist/**',
      'svelte/scripts/**',
      'tests/e2e/**',
      'coverage/**'
    ]
  },
  js.configs.recommended,
  ...tseslint.configs.recommendedTypeChecked,
  {
    // Wave rule (phase 2 section 0): >50 findings, dropped. The bulk is
    // .svelte importing .svelte — a module form TypeScript cannot resolve.
    rules: {
      '@typescript-eslint/no-unsafe-member-access': 'off',
      '@typescript-eslint/no-unsafe-call': 'off',
      '@typescript-eslint/no-unsafe-assignment': 'off',
      '@typescript-eslint/no-unsafe-argument': 'off'
    }
  },
  {
    // .svelte script blocks that import other .svelte files: TypeScript
    // cannot resolve those modules, so the imported values are error-typed
    // and these two rules only report that artifact where it hits.
    // The store test files consume store.svelte's named exports directly.
    files: [
      '**/*.svelte',
      'svelte/lib/store.svelte.test.ts',
      'svelte/lib/store.window.test.ts',
      'svelte/lib/store.commands.test.ts',
      'svelte/lib/store.sessions.test.ts'
    ],
    rules: {
      '@typescript-eslint/no-unsafe-return': 'off',
      '@typescript-eslint/no-redundant-type-constituents': 'off'
    }
  },
  {
    // Type-aware rules need parser services: the project service resolves
    // each file's tsconfig (v8 has no default for it).
    files: ['**/*.ts'],
    languageOptions: { parserOptions: { projectService: true } }
  },
  ...sveltePlugin.configs['flat/recommended'],
  {
    // The type-checked preset applies to every file, but the .mjs config
    // files have no type information — the typed rules crash on them, so
    // the typed subset is off there (the untyped rules still apply).
    files: ['**/*.mjs'],
    rules: {
      '@typescript-eslint/await-thenable': 'off',
      '@typescript-eslint/no-array-delete': 'off',
      '@typescript-eslint/no-base-to-string': 'off',
      '@typescript-eslint/no-duplicate-type-constituents': 'off',
      '@typescript-eslint/no-floating-promises': 'off',
      '@typescript-eslint/no-for-in-array': 'off',
      '@typescript-eslint/no-implied-eval': 'off',
      '@typescript-eslint/no-misused-promises': 'off',
      '@typescript-eslint/no-redundant-type-constituents': 'off',
      '@typescript-eslint/no-unnecessary-type-assertion': 'off',
      '@typescript-eslint/no-unsafe-argument': 'off',
      '@typescript-eslint/no-unsafe-assignment': 'off',
      '@typescript-eslint/no-unsafe-call': 'off',
      '@typescript-eslint/no-unsafe-enum-comparison': 'off',
      '@typescript-eslint/no-unsafe-member-access': 'off',
      '@typescript-eslint/no-unsafe-return': 'off',
      '@typescript-eslint/no-unsafe-unary-minus': 'off',
      '@typescript-eslint/only-throw-error': 'off',
      '@typescript-eslint/prefer-promise-reject-errors': 'off',
      '@typescript-eslint/require-await': 'off',
      '@typescript-eslint/restrict-plus-operands': 'off',
      '@typescript-eslint/restrict-template-expressions': 'off',
      '@typescript-eslint/unbound-method': 'off'
    }
  },
  {
    files: ['**/*.svelte', '**/*.svelte.js', '**/*.svelte.ts'],
    languageOptions: {
      globals: globals.browser,
      parser: svelteParser,
      parserOptions: {
        projectService: true,
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
