# Research: Svelte 5 + TypeScript practices for the frontend refactor (2026, primary sources)

Researched 2026-10-01, in preparation for the post-overhaul refactor. Scope: enforceable Svelte 5
runes/component/TS practices for `app/svelte/` (Tauri-embedded SPA, no SvelteKit, no SSR) plus
static-analysis tooling beyond the already-adopted set (ESLint 10 flat + `eslint-plugin-svelte`
`flat/recommended` + `@stylistic`, svelte-check 4.7.6, vitest 5 coverage, protocol-parity script).
Stack as installed: svelte 5.57.1, typescript 6.0.3 (+ `@typescript/native` 7.0.2 for
`svelte-check --tsgo`), vite 8.3.1. Every claim below is cited to the source fetched during this
task; nothing is from memory.

**Short answer.** The codebase is already fully runes-idiomatic (census: 57 `$derived`, 18 `$state`,
13 `$effect`, 6 `$props`; zero `svelte/store` imports, zero context) — the refactor is not a
migration, it is **discipline enforcement**. Three findings drive it: (1) the official `$effect`
docs define the exact rule the repo's 13 effects must obey — effects are an escape hatch for the
outside world, never for synchronising state; (2) the official TS docs for Svelte recommend two
`tsconfig` flags this repo lacks (`verbatimModuleSyntax`, `isolatedModules`), and the transcript's
id-keyed maps are precisely the case `noUncheckedIndexedAccess` exists for; (3) the one real
tooling gap is **type-aware typescript-eslint** (`recommendedTypeChecked` + `projectService`),
which the current config does not run — everything else in the candidate space is either already
enforced by `flat/recommended` or experimental.

## 1. Runes discipline

The first-party position, from the Svelte 5 announcement: Svelte 4's `$:` "conflated two concepts
(derived state and side-effects) that really should be kept separate, and because dependencies are
determined when the statement is compiled (rather than when it runs), it resists refactoring and
becomes a magnet for complexity" — runes exist to remove that footgun
(https://svelte.dev/blog/svelte-5-is-alive). The docs then define what each rune is *for*; the
enforceable rules follow directly.

**1.1 `$effect` is for the outside world only — never for synchronising state.** The `$effect`
docs: "In general, `$effect` is best considered something of an escape hatch — useful for things
like analytics and direct DOM manipulation — rather than a tool you should use frequently. In
particular, avoid using it to synchronise state" (the derived replacement is shown in the same
section; `$derived.by` for anything more complex than a one-line expression)
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/04-$effect.md, "When not
to use `$effect`"). The opening paragraph is stronger: "you should _not_ update state inside
effects, as it will make code more convoluted and will often lead to never-ending update cycles".
The one sanctioned exception: "If you absolutely have to update `$state` within an effect and run
into an infinite loop because you read and write to the same `$state`, use `untrack`" (same page).
**Candidate rule (adopt):** every `$effect` must be reviewable as "talks to the outside world"
(DOM, timers, third-party libs, logging); a `$effect` whose body assigns app state is a
`$derived` in disguise. Of the repo's 13 `$effect` sites (Transcript, Composer, SessionNode,
LeftPane, RightPane, WorkspaceTabs, EntryCard, mock-store), each should be checked against this
test during the refactor.

**1.2 The mechanical half is already enforced.** `svelte/prefer-writable-derived` is in
`plugin:svelte/recommended` (which the repo runs via `flat/recommended`) and reports exactly the
single-assignment case: "$state() initialisation + `$effect`/`$effect.pre` assigning a new value
to that same variable, where the effect body is a single assignment" — with an auto-fix suggestion
to `$derived` (https://github.com/sveltejs/eslint-plugin-svelte/blob/main/docs/rules/prefer-writable-derived.md).
Multi-statement effects that write state are **not** caught mechanically — that is the review-time
half of rule 1.1. **Verdict: no new tooling; the rule above is the delta.**

**1.3 App state lives in `.svelte.ts`/`.svelte` modules; exported state is never directly reassigned.**
Runes only transform in `.svelte` and `.svelte.js`/`.svelte.ts` files (https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/01-what-are-runes.md)
— which is why the repo's `lib/store.svelte` is a `.svelte` module. The cross-module constraint:
"You can declare state in `.svelte.js` and `.svelte.ts` files, but you can only _export_ that
state if it's not directly reassigned" — an exported-and-reassigned binding leaks its raw signal
to importers ("`console.log(typeof count)` // 'object', not 'number'")
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/02-$state.md, "Passing state across modules").
The store's current shape (export the `store` object binding; mutate through methods) is exactly
the sanctioned pattern. **Candidate rule (adopt, codifies existing behaviour):** new shared state
goes in a `.svelte.ts` module exporting an object binding; functions mutate internals, never
reassign the export.

**1.4 Classes with `$state` fields: the `this` footgun.** "Class instances are not proxied.
Instead, you can use `$state` in class fields" — but "This won't work, because `this` inside the
`reset` method will be the `<button>` rather than the `Todo`: `<button onclick={todo.reset}>"`;
the fixes are an inline arrow `onclick={() => todo.reset()}` or an arrow-function field on the
class (https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/02-$state.md, "Classes").
**Candidate rule (adopt):** method references passed as event handlers for `$state`-field classes
are wrapped in arrows; the class defines arrow fields where the method needs `this`.

**1.5 Reads after `await` are not tracked.** "Values that are read _asynchronously_ — after an
`await` or inside a `setTimeout`, for example — will not be tracked"
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/04-$effect.md, "Understanding dependencies").
Relevant to the transcript: an effect that reads store state only inside an `await`-ed continuation
silently loses the dependency. **Candidate rule (adopt, review-time):** reactive reads in
`$effect`/`$derived` happen synchronously; values needed after an `await` are captured into a
local before the `await`.

**1.6 Derived values are writable (5.25+) — the optimistic-UI idiom.** "you can temporarily
override their values by reassigning them (unless they are declared with `const`)… useful for
things like _optimistic UI_… Prior to Svelte 5.25, deriveds were read-only"
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/03-$derived.md, "Overriding derived values").
The repo is on 5.57.1, so `let x = $derived(...)` + reassignment is available where a pending
command round-trip wants immediate feedback (ADR-0008's upsert flow is the natural candidate).
**Verdict: pattern note for the refactor, not an AGENTS.md rule** — it is an idiom, not a
discipline; overusing it would re-create the state-sync mess rule 1.1 bans.

## 2. Component architecture

**2.1 Snippets, not slots.** "Svelte 5 replaces them [slots] with snippets, which are more
powerful and flexible, and so slots are deprecated in Svelte 5. They continue to work" — and the
`children` prop is reserved for the default snippet
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/07-misc/07-v5-migration-guide.md, "Snippets instead of slots", "The `children` prop is reserved").
The announcement gives the design reason: Svelte 4 "treats event handlers and 'slotted content'
as separate concepts, distinct from the props that are passed to components… This was a mistake"
(https://svelte.dev/blog/svelte-5-is-alive). **Candidate rule (adopt):** new/refactored
components take content as snippet props (`{#snippet name}…{/snippet}` → `{@render name()}`);
`<slot>` is a Svelte 4 relic. No ESLint rule enforces this (the plugin's slot rules are
`no-dynamic-slot-name`, `experimental-require-slot-types` — none bans slots; full rule list:
https://github.com/sveltejs/eslint-plugin-svelte/blob/main/README.md), so it is a review-time
rule. A repo-wide grep found no `<slot>` in `app/svelte/components/` — the codebase already
complies; the rule protects the refactor.

**2.2 `onX` props, not `on:`.** "In Svelte 5 [event handlers] are properties like any other (in
other words — remove the colon)", with the shorthand `{onclick}` form for named functions
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/07-misc/07-v5-migration-guide.md, "Event changes").
The components already use `onclick={…}` throughout. **Verdict: already idiomatic — no rule
needed beyond not regressing to `on:` in new code** (a `svelte/` lint rule would be
nice-to-have; none exists in the current plugin, so this stays a convention noted in the
refactor brief, not AGENTS.md).

**2.3 Type-annotate every `$props()` destructuring.** The official docs: "You can add type
safety to your components by annotating your props, as you would with any other variable
declaration… Adding types is recommended, as it ensures that people using your component can
easily discover which props they should provide"
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/02-runes/05-$props.md, "Type safety").
The repo already does this inline (`let { entry, ws, depth = 0 }: { entry: FileEntry; … } =
$props()`). **Verdict: adopt as a stated rule** — it is the first-party recommendation and the
refactor will touch every component; untyped props are the one props-footgun the docs name.

**2.4 Context: skip, with the reason on record.** The one risk the context docs attach to
module-level shared state is SSR: "if you mutate the state during server-side rendering… the data
may be accessible by the _next_ user. Context solves this problem because it is not shared
between requests" (https://github.com/sveltejs/svelte/blob/main/documentation/docs/06-runtime/02-context.md,
"Replacing global state"). Tau is a single-window desktop SPA with no SSR, so the cited risk does
not apply and the `.svelte.ts` module store (rule 1.3) is the sanctioned pattern; context adds
indirection without buying anything here. **Verdict: skip** — no rule, no migration; documented so
the refactor doesn't "fix" the store into context.

## 3. TypeScript strictness

The repo's `app/tsconfig.json` has `strict: true` and nothing beyond it. The official Svelte TS
docs recommend two flags it lacks: "Set `verbatimModuleSyntax` to `true` so that imports are left
as-is" and "Set `isolatedModules` to `true` so that each file is looked at in isolation.
TypeScript has a few features which require cross-file analysis and compilation, which the Svelte
compiler and tooling like Vite don't do"
(https://github.com/sveltejs/svelte/blob/main/documentation/docs/07-misc/03-typescript.md, "tsconfig.json settings").

**3.1 `verbatimModuleSyntax` + `isolatedModules` (adopt).** Both are first-party recommendations
for Svelte+TS (source above), both are free (they can only surface existing errors), and
`isolatedModules` is the one that protects against the whole-file-analysis features Svelte's
per-file compilation cannot handle.

**3.2 `noUncheckedIndexedAccess` (adopt — the flag the transcript was built for).** "Turning on
`noUncheckedIndexedAccess` will add `undefined` to any un-declared field in the type"
(https://www.typescriptlang.org/tsconfig, "No Unchecked Indexed Access"). The central-column store
is "an id-keyed map" of transcript entries (`lib/store.svelte` header comment; spec §8 paged
reads) — index access into an id-keyed map is exactly the `T | undefined` case this flag models,
and the hydration path (rebuild from snapshot + events, ADR-0008) is where a silently-`undefined`
entry becomes a render bug. Cost: a bounded wave of `| undefined` handling at index sites, all
mechanical.

**3.3 `noFallthroughCasesInSwitch` (adopt, cheap).** "Report errors for fallthrough cases in
switch statements… so you won't accidentally ship a case fallthrough bug"
(https://www.typescriptlang.org/tsconfig, "No Fallthrough Cases In Switch"). The protocol-parity
surface (ADR-0006) means command/event folding lives in switches over discriminated unions —
this flag is directly on the critical path.

**3.4 `exactOptionalPropertyTypes` (defer).** It "makes TypeScript truly enforce the definition
provided as an optional property" — `prop: undefined` is no longer assignable where the type is
`"dark" | "light"` (https://www.typescriptlang.org/tsconfig, "Exact Optional Property Types").
The protocol mirror types carry many `?` fields, so enabling it forces `| undefined` into the
shared protocol surface — a cross-language change (the Rust side's `Option` already means
"absent"), not a frontend-local one. **Verdict: defer past this refactor**; it is a protocol
decision, not a Svelte one.

**3.5 `noUnusedLocals`/`noUnusedParameters` (skip).** "Report errors on unused local
variables/parameters" (https://www.typescriptlang.org/tsconfig, "No Unused Locals" / "No Unused
Parameters") — already covered: the repo runs `@typescript-eslint/no-unused-vars` with
`^_` ignore patterns (app/eslint.config.mjs). Enabling the compiler flags too would double-report
the same findings; **skip** to keep one source of truth for the rule.

## 4. Tooling beyond the adopted set

**4.1 Type-aware typescript-eslint (adopt — the one real gap).** The repo's ESLint runs
`tseslint.configs.recommended` (untyped). typescript-eslint's type-checked tier "utilize[s]
TypeScript's type checking APIs to provide much deeper insights… These rules are slower than
traditional lint rules but are much more powerful", enabled by "Add `TypeChecked` to the name of
any preset configs… `recommendedTypeChecked`" + `languageOptions.parserOptions.projectService:
true` (https://typescript-eslint.io/getting-started/typed-linting/). Cost, from the first-party
performance page: "your lint times should be roughly the same as your build times"
(https://typescript-eslint.io/troubleshooting/typed-linting/performance/). The Svelte plugin's own
README documents the Svelte-specific wiring — `.svelte` files routed through the TS parser with
`parserOptions: { projectService: true, extraFileExtensions: ['.svelte'], parser: ts.parser,
svelteConfig }` (https://github.com/sveltejs/eslint-plugin-svelte/blob/main/README.md) — so the
`.svelte` script blocks get typed rules, not just the `.ts` modules. What it unlocks is the
class of rule untyped lint cannot see: floating promises in the command/event handlers,
unnecessary type assertions, `any`-tainted template expressions. **Verdict: adopt** — CI lint is
currently a small fraction of the frontend job's time, so "lint ≈ build" is affordable, and it
closes the largest remaining static-analysis gap.

**4.2 The plugin's experimental rules (skip for now).** `experimental-require-slot-types` and
`experimental-require-strict-events` carry the experimental prefix in the current 3.23.0 rule
set (https://github.com/sveltejs/eslint-plugin-svelte/blob/main/README.md — experimental rules
are outside `recommended`). Revisit when they stabilise; adopting experimental rules into a CI
gate now trades a small gain for churn risk.

**4.3 Anything else (skip — none found).** The candidate space was checked against the adopted
set: a second linter is off the table (Biome's Svelte support is experimental with a
file-corrupting formatter regression in the current release — `docs/research/testing-frontend-e2e.md`
§2, still true), svelte-check is the type gate already, and no other first-party tool targets
Svelte 5 runes specifically. The `flat/recommended` config already carries the mechanical runes
discipline: `prefer-writable-derived`, `prefer-derived-over-derived-by`,
`no-unnecessary-state-wrap`, `no-inspect`, `prefer-svelte-reactivity` (deliberately off here —
plain Maps in the store, documented in the config), `no-useless-children-snippet`,
`no-at-const-tags`, `require-each-key`, `valid-each-key`, `require-event-dispatcher-types`,
`no-unused-props`, `no-unused-svelte-ignore` (rule table:
https://github.com/sveltejs/eslint-plugin-svelte/blob/main/README.md).

## Verdict — candidate AGENTS.md rules

1. **`$effect` is for the outside world** (DOM, timers, third-party libs, logging) — never for
   synchronising app state; that is `$derived`. Async continuations capture reactive reads before
   the `await`. (Sources: §1.1, §1.5)
2. **Shared state lives in `.svelte.ts` modules** exporting an object binding; mutations go
   through methods, exports are never reassigned. No context, no `svelte/store` — the single-
   window SPA makes the module store the sanctioned pattern. (Sources: §1.3, §2.4)
3. **Components**: snippets over slots; `onX` props over `on:`; every `$props()` destructure
   type-annotated. (Sources: §2.1–2.3)
4. **tsconfig**: `strict` + `verbatimModuleSyntax` + `isolatedModules` +
   `noUncheckedIndexedAccess` + `noFallthroughCasesInSwitch`; `exactOptionalPropertyTypes`
   deferred (protocol decision); no `noUnusedLocals` (ESLint owns it). (Sources: §3.1–3.5)
5. **Lint tier**: add `recommendedTypeChecked` + `projectService` to the ESLint config
   (type-aware rules for `.ts` and `.svelte` script blocks). (Source: §4.1)

Open item for the refactor brief (not an AGENTS.md rule): audit the 13 existing `$effect` sites
against rule 1 and the `<slot>`-free / `onX` idioms against rules 2–3 as each component is
touched; the optimistic-UI derived-override idiom (§1.6) is available (svelte 5.57.1 ≥ 5.25) for
the pending-command feedback case.
