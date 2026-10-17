# Workbench GUI plan — the session-first frontend

2026-10-12. Inputs: the workbench prototype (`app/prototype-workbench.html`, commit 3a809de,
checkpoint tag `checkpoint/prototypes-2026-10-12`) and its design note
(`docs/research/prototype-workbench.md`); spec §9 (`docs/spec/v0.md`); the binding usage model
(**one session + 3–4 sub-agents** is the hero case; heavier fleets are a scale-up, not a
re-design). The prototype is the *specification* for the rebuild — a throwaway HTML file.
Nothing is ported line-for-line; every piece below is built in Svelte against the protocol.

## What we are building

The current app already has the spec's §9 skeleton (tabs top, sessions left, transcript
central, dock right, focus mode, lualine bar). The rebuild is **depth on that skeleton**, in
priority order:

1. **The transcript done exceptionally** — live sub-agent blocks inline in the parent
   session, report cards, waiting-on-you blocks, system dividers, follow-to-isolate.
2. **The crew** — a live sub-agents dock with per-worker state, tokens, latest line, follow.
3. **Attention** — bell + short sheet + tints; a GUI-side fold, no protocol change.
4. **Files + peek diff** — the one protocol extension in this plan (git status/diff).
5. **Step-outs** — command palette, fleet list, focus-mode edge rails + inspector.
6. **Skin + queue semantics + status bar** — the workbench's visual language, end to end.

The prototype's fleet paradigms (board columns, feed, concierge) are **out of scope** — they
remain step-out ideas for post-v0 if orchestration ever earns it.

## Current state (what we build on)

- Shell: `App.svelte` (280/1fr/330 grid, focus = bare grid collapse).
- `Transcript.svelte` (virtua virtualization, stick-to-bottom), `EntryCard.svelte`,
  `LeftPane` + `SessionNode` (MRU tree, sub-agent nesting, badges), `RightPane`
  (tasks | sub-agents), `Queue`, `Composer` (3-lane + model chip), `StatusBar`,
  `WorkspaceTabs`, `ModelMenu`.
- `lib/`: `store.svelte` (729 LOC), `protocol.ts` (485), `presentation`, `sessions`,
  `entries`, `windowing`, `commands`, `autocomplete`, `files`, `format`, `markdown`,
  `panes`, `selection`.
- Protocol already carries everything phases 1–3, 5 (attention fold) and 6 need:
  `SubagentInfo` (state, `waiting_on`, `last_message`, `usage`, `task`,
  `resume_contract`), `subagent_event` (`spawned`/`state`/`notified`),
  `session_entries` (a child is an ordinary session — its entries are readable),
  `session_list` per workspace, `LiveState.subagents`.
- Test regime: Vitest + Testing Library (colocated `*.test.ts`), nextest, svelte-check,
  ESLint, 500/1000-LOC gates, `accept-*` mock-only suites, WebDriverIO e2e legs
  (`TAU_E2E_MODE`: stress / mock).

## Protocol decisions

**One deliberate IPC addition** (reviewable per the command-surface rule):

- `git_status { workspace }` → per-path status (M/A/D/R) for the file tree.
- `git_diff { workspace, path }` → unified diff text for the peek sheet.

Rust side shells out to `git` (the harness already assumes git; no new crate). Both are
read-only, workspace-scoped, and return typed payloads. Non-git workspace → empty status,
no diff. This is the *only* Rust work in the plan; it lands in phase 4 with its own
protocol tests (Rust + TS mirror).

**No changes** for: attention (GUI fold over session/sub-agent state), follow (GUI state),
fleet list (existing `session_list`), palette (GUI commands), inline sub-agent blocks
(`SubagentInfo` + `subagent_event` + child `session_entries`).

## Phases

One branch per phase, sequential — each lands CI-green (lint, types, tests, coverage
report) before the next starts. The prototype screenshots
(`/tmp/workbench-*.png`, re-producible by opening the HTML file) are the visual reference
for every phase.

### Phase 1 — Skin (no behavior change)

Expand `app.css` with the workbench token set: radius scale (6/10/14), shadow scale,
state colors (running=green, needs=red, idle=dim, done=faint), chip styles, divider
styles. Restyle, in place: `WorkspaceTabs`, `LeftPane`/`SessionNode`, `RightPane`,
`Queue`, `Composer`, `StatusBar`, `EntryCard`. No new components, no store changes.

- DoD: full frontend suite green; a side-by-side screenshot pass (dev build via
  tauri-pilot) against the prototype shots shows the same visual language.

### Phase 2 — Transcript depth (the heart)

- **System dividers** as first-class entry renderings: spawn, wake, queue drain
  (steering/follow-up delivery), compaction. Distinct tints per kind, one line each.
- **`SubagentBlock.svelte`** — the signature piece. Rendered inline in the parent
  transcript at the spawn point. Shows: state-coloured edge, name, `agent_type`,
  context-mode chip, live latest line (from `SubagentInfo.last_message`, refreshed by
  `subagent_event`), tokens/elapsed, `follow` button; click opens the child session.
  Live while running; collapses to its final state when done.
- **Report card** — on `done`, a green in-thread card carrying the
  schema-validated `parent_notify` output.
- **Waiting-on-you block** — red-bordered, for `idle` + `waiting_on: user` (child) and
  the parent's own parked questions; the single most important affordance in the app.
- **Follow mode** — `store.follow: handle | null`; while set, non-followed entries render
  at reduced opacity and a chip pins in the transcript header ("following: X ✕").
- **New-activity pill** — when scrolled up and new entries arrive, a floating pill
  ("↓ n new") instead of yanking the scroll position.

- DoD: block/follow/divider logic unit-tested against the mock store (spawn → running →
  done sequence drives the block through all states); Transcript stays under the LOC gate
  (virtualization container only — blocks are `EntryCard` siblings).

### Phase 3 — Crew dock

`RightPane` sub-agents tab becomes the live crew: one row per `SubagentInfo` — state
dot, name, `agent_type`, context chip, tokens, elapsed, latest line (truncated),
hover actions (follow / open). Running first, then waiting, then terminal states.
The tasks tab is unchanged (already spec-shaped).

- DoD: component tests with a 4-worker fixture (the hero case) covering the sort,
  the live-line refresh, and follow/open wiring.

### Phase 4 — Files + peek diff (the protocol extension)

- Rust: `git_status` / `git_diff` commands (see Protocol decisions) + tests.
- TS: protocol mirror + `commands.ts` wrappers + store plumbing.
- Files tab: git badges (M/A/D, amber/green/red) in the tree.
- **Peek sheet** — non-modal, 470px, anchored to the right of the center column
  (a child of `.center`, *not* the body — the prototype's bug #2), unified diff with
  red/green lines, file switcher, close on Esc/×. The center is always the transcript.
- Absorbs the diff-viewer half of open issue #116.

- DoD: Rust + TS protocol tests (positive/negative: git repo, non-git workspace,
  missing path); component test for the peek sheet; e2e-observable via mock leg.

### Phase 5 — Attention layer

- `lib/attention.ts` — pure fold: given workspaces' sessions + live sub-agent state,
  produce the needs-you list (session parked on a user question; child
  `waiting_on: user`). Unit-tested as a pure function.
- Bell + count in the top bar; **attention sheet** (overlay, short list): name,
  workspace, reason line, elapsed, jump. Jump = open the session (workspace switch
  included). Red tint on attention rows in the session tree.

- DoD: fold tests (multi-workspace fixture incl. a cross-workspace item); sheet
  component test (open → jump closes and navigates).

### Phase 6 — Step-outs: palette, fleet, focus rails

- **Command palette** (⌘K): grouped items (open session, follow crew, new session,
  attention, fleet, focus, stop, archive…), filter-as-you-type, j/k + Enter, Esc.
- **Fleet list** (⌘\): compact overlay of every session across workspaces — state dot,
  name, workspace, latest line, tokens; click jumps. A list, deliberately.
- **Focus mode, upgraded**: edge pill rails (left: sessions; right: crew/tasks/files)
  that open **inspector sheets** (non-modal, beside the rail) instead of nothing.
- **Keyboard map**: j/k session nav in the left pane, ⌘. attention, F focus,
  r (existing), ⌘N (existing), Esc cascade (palette → sheet → peek → inspector →
  follow).

- DoD: palette/fleet/inspector component tests; keymap covered by an e2e leg step.

### Phase 7 — Queue semantics, status bar, acceptance

- Queue lanes gain delivery-timing labels ("next tool boundary" / "after work
  completes"); items remain deletable.
- Status bar to the workbench lualine layout: state dot, workspace · session, OM
  gauge (obs/threshold + pending), tokens in/out, cache %, tps while streaming, cost.
- **E2E**: extend the mock leg with the workbench journey (spawn → live block appears →
  follow isolates → done → report card → bell for the parked sibling → answer →
  running); stress leg unchanged (10k entries must stay smooth with blocks in the
  stream — the acceptance bar).
- Visual walkthrough of the dev build (tauri-pilot, screenshots read as images);
  rough edges fixed; plan doc updated with what the real build confirmed or changed.

- DoD: all `accept-*` green, e2e legs green, walkthrough notes recorded.

## Scale-up path (unchanged by fleet size)

Nothing in the home view is fleet-shaped: the session list, the crew list, the attention
sheet, and the fleet step-out are all *lists*. A user with 20 sub-agents gets longer
lists; the hero 1 + 3–4 case is pixel-identical. Board/feed/concierge remain post-v0
experiments.

## Risks

- **Virtua + inline blocks**: the transcript's virtualized item model gains block rows.
  Mitigation: blocks are ordinary entries in the rendered list (they have stable ids —
  the child handle), so windowing is untouched; verify in the stress leg.
- **`last_message` freshness**: the crew row's live line depends on the core emitting
  `subagent_event state` with a useful `note`/`last_message`. If the current payload is
  too sparse, phase 3 opens a small core ticket (event enrichment) rather than hacking
  the GUI.
- **Skin drift**: phase 1 touches every component's CSS. Mitigation: token-first
  (components reference tokens, never raw values), screenshot pass before merge.
- **Scope creep toward the fleet paradigms**: the prototype's board/feed/concierge are
  explicitly out; any ticket touching them is rejected back to post-v0.

## Tickets

Map: #139 "Workbench GUI — the session-first frontend" (`wayfinder:map`), one child
ticket per phase, each blocked by its predecessor:

| Phase | Ticket |
|---|---|
| P1 skin | #140 |
| P2 transcript depth | #141 |
| P3 crew dock | #142 |
| P4 files + peek diff | #143 |
| P5 attention | #144 |
| P6 step-outs | #145 |
| P7 queue/status/acceptance | #146 |

#116 is linked from P4 (diff viewer half) and stays open for its file-tree half
(show-all, gitignore greying, icons).
