# Section 4 brief — Frontend leaf modules (phase 2)

You are the worker for section 4 of the phase 2 refactor. Read this brief fully before
touching anything. Context: `docs/overhaul/phase2-plan.md` §4; the F3/F4 candidate cards in
`docs/overhaul/architecture-review-20261001.html`; conventions in `AGENTS.md` (Coding
conventions + Testing); Svelte/TS first-party rules in `docs/research/refactor-svelte.md`.

This is a **pure extraction** section: no behaviour changes. The 336 existing component
tests are your safety net and must pass unmodified throughout.

## Worktree (mandatory)

Another worker is active in the main repo. You must not touch its working tree.

```
git -C /Users/aaron/git/tau worktree add /tmp/tau-p2-s4 -b phase2/section4-leaf-modules main
```

Do all work in `/tmp/tau-p2-s4`. Never edit files under `/Users/aaron/git/tau` directly
(the `git worktree add` command itself is the only main-repo operation you run, plus the
final `git worktree remove` if you abort).

Your branch is based on main **without** section 0 (which is in flight). Write code that
already satisfies section 0's incoming rules, so the post-merge rebase is clean:
- `noUncheckedIndexedAccess` semantics: index access on id-keyed maps/arrays is `T | undefined` — handle it.
- type-aware ESLint semantics: no floating promises, no `any`, no unnecessary assertions.
- `$props()` destructures type-annotated; snippets over slots; `onX` over `on:`.

## File ownership (hard boundary)

You may modify **only**:
- `app/svelte/lib/entries.ts`, `app/svelte/lib/markdown.ts`, `app/svelte/lib/panes.ts`
- `app/svelte/lib/presentation.ts` (new), `app/svelte/lib/selection.ts` (new)
- `app/svelte/components/EntryCard.svelte`, `SessionNode.svelte`, `LeftPane.svelte`
- test files colocated with the above (`*.test.ts`)

If you discover you need any other file (especially `store.svelte`, `Transcript.svelte`,
`commands.ts` — section 3 territory), **stop and report it** in your final message
instead of touching it.

## Work items

### 1. F3 — entry presentation module (`lib/presentation.ts`)

Today the per-kind presentation logic (label, icon, kv rows, which sections render, kind
specifics) is a 10-branch ladder smeared across `entries.ts` decoding, `markdown.ts`
rendering, and `EntryCard.svelte` (814 LOC). Move the kind knowledge into one pure module:

```ts
present(entry: Entry): Presentation
// Presentation = { label, icon, rows: KvRow[], sections: Section[] }
```

- `EntryCard.svelte` becomes a **generic renderer** over the `Presentation` shape — no
  per-kind branching left in the component.
- What counts as "kind knowledge": which label/icon per kind, which payload fields become
  rows, which sections (markdown, diff, output, etc.) a kind renders. What stays in the
  component: layout, markup, event wiring.
- `markdown.ts`/`entries.ts` keep their current roles (render markdown / decode entries);
  the module composes them. Don't restructure them beyond what the extraction needs.
- Tests (new, `lib/presentation.test.ts`): positive per representative kind (message,
  tool call, task, observation — whatever the card supports) and negative: an unknown/
  malformed entry kind degrades to a sensible fallback (assert the shape, not a throw,
  unless the current behaviour throws — preserve current behaviour).

### 2. F4 — one selection rule (`lib/selection.ts`)

Today the multi-select interaction (cmd/ctrl click toggles, shift click selects the range
from the anchor, plain click clears/sets single) is implemented twice: pure in
`panes.ts` (`applyTabSelect`) and inline in `SessionNode.svelte` (`onRowClick`). Extract
one pure function, parameterized by the ordered id sequence:

```ts
nextSelection(orderedIds: string[], current: string[], clickedId: string,
  mod: { shift: boolean; multi: boolean }): string[]
```

- `applyTabSelect` and `onRowClick` become thin adapters over it (they keep their
  event-plumbing roles; the rule lives in one place).
- Preserve the exact current semantics of **both** variants; where the two variants
  subtly differ, keep the difference expressible via the parameters (report any
  difference you find in your final message).
- Tests (new, `lib/selection.test.ts`): positive + negative per interaction — single
  click, multi toggle on/off, shift range forward/backward, click outside current
  selection, empty selection, single-item list. Assert the resulting id arrays.

## Conventions

- AGENTS.md applies: comment discipline (no narration), 2-space indent, single quotes,
  semicolons, files < 500 LOC soft limit, tests are code.
- `EntryCard.svelte` is 814 LOC today; after the extraction it should be smaller. If any
  file you produce exceeds 500 LOC, split it — do not carry the bloat forward.
- Commit per work item: `F3 presentation module`, `F4 selection rule` (tests included
  in each commit).

## Verification (all must pass before your final message)

In `/tmp/tau-p2-s4`:
1. `cd app && npx eslint .` (current config — section 0's typed tier isn't on your branch;
   write clean code per the pre-adoption rules above)
2. `cd app && npx svelte-check`
3. `cd app && npx vitest run` — 336 existing + your new tests, zero failures
4. `cd app && npx vite build`
5. `just acceptance` from the worktree root — 6/6 (no core changes expected, but the
   front end ships in the app)

## Final message

Per work item: what moved where, LOC before/after for EntryCard, the (possibly empty)
list of variant differences you found in F4, new test counts, files changed, verification
results per command, and anything you had to stop short of (especially the ownership
boundary).
