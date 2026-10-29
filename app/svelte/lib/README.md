# lib

Five directories; the directory answers the two questions this frontend's readers ask:
where does app state live, and which modules are pure.

| directory | what lives here | imports |
| --- | --- | --- |
| `wire/` | the protocol mirror: `protocol.ts` (the `Command`/`Event`/entry shapes, kept in step with `crates/tau-protocol` by `scripts/protocol-parity.mjs`), `entries.ts` (the one `ViewEntry` decoder), `stream.svelte.ts` (the live-delta clock), `errors.ts` | nothing above itself |
| `rules/` | the pure rules the store applies — registry invariants (`sessions.ts`), pane layout (`panes.ts`), the listing cache (`files.ts`), the open-file list (`openfiles.ts`), the tab multiselect (`selection.ts`), the paged-hydration policy (`windowing.ts`) | `wire/` |
| `store/` | the app state: `state.svelte` owns the one `$state` bag, the six feature modules each own a round-trip, `commands.ts` the session-command cluster, `store.svelte` the facade every component imports | `wire/`, `rules/`, `display/` |
| `display/` | data in, display out: `presentation.ts` maps an entry to a card shape, plus `plan`, `subagent-view`, `attention`, `palette`, `diffview`, `markdown`, `highlight`, `format`, `diagnostics`, `autocomplete`, `ui`, `useContextMenu`, `externalLinks`; `presentation.svelte` is the exception — the display preferences, persisted to localStorage | `wire/`, `rules/` |
| `testing/` | the shared suites' harness: `store-harness.ts` fixtures, `mock-store.svelte.ts`, and the component-test fixtures `VirtualizerMock.svelte`, `TreeNodeFixture.svelte` | anything |

Dependency direction is `wire` → `rules` → `display` → `store` → `components`, acyclic inside
`lib` and checkable by an import scan. One edge crosses the rows upward: `wire/entries.ts`
renders a payload's trailing JSON with `display/markdown.ts`'s `valueLinesOf` — the decoder
owns the layout of a prose payload, so the rendering helper sits below it. The store's reads
of `display/` (the preferences in `presentation.svelte`, the `attention.ts` fold) are ordinary
store→display edges.

Test suites sit beside the group they exercise, named after the module they pin when one
module carries the behavior (`store/events.test.ts` pins `store/events.svelte.ts`); suites
that drive the facade's surface are named after the behavior (`store/surface.test.ts`,
`store/session-archive.test.ts`).
