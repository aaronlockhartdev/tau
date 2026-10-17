# Prototype: Workbench (session-first)

Single-file prototype: `app/prototype-workbench.html`.

## The idea

The fourth prototype, and the one the usage model actually asks for. The other three
optimized for the orchestrator — **board v2** (space: fleet columns), **pulse** (time: event
stream), **concierge** (dialogue). All three put the *fleet* at the center. But the real day is
**one session + 3–4 worker sub-agents**, and your attention lives in one live transcript. So
workbench puts the session at the center and makes the fleet a set of *step-out* views.

```
┌────────────────────────────────────────────────────────────────────────────┐
│  ⚡ tau  bloody-chairs  pi  +                        🔔1   ⛶   Search ⌘K  │
├──────────────┬──────────────────────────────────────────────┬──────────────┤
│ SESSIONS     │  CSS architecture review  [tau]  RUNNING     │ Crew  Tasks  │
│ Sessions Files│                                             │ Files        │
│ ┌ TAU · 4 ┐  │  ── 65 earlier entries · compacted ──       │ ┌──────────┐ │
│ │ ▸ CSS arch ▸ researcher        DONE                      │ │researcher│ │
│ │          ▸ reviewer      RUNNING                         │ │ RUNNING  │ │
│ │          ▸ coder         RUNNING                         │ │◦ edit …  │ │
│ │          ▸ style-audit   DONE                            │ ├──────────┤ │
│ │   Wayfinder  NEEDS                                       │ │  coder   │ │
│ └──────────┘│  [transcript: reasoning, tool cards,         │ │ RUNNING  │ │
│  …          │   sub-agent blocks, report cards, stream]    │ └──────────┘ │
├──────────────┴──────────────────────────────────────────────┴──────────────┤
│ ● running · tau · CSS arch · OM 9.8k/30.0k · ↑45.7k · 18 tps · $0.31      │
└────────────────────────────────────────────────────────────────────────────┘
   step-outs: ⌘. attention sheet · ⌘\ fleet list · F focus mode · ⌘K palette
```

## What "better than v1" means, concretely

v1 (`prototype-board.html`, 908 LOC) had the right skeleton — sessions left, transcript
center, dock right, precision skin. Workbench keeps the skeleton and the skin, and fixes
what the v2 research doc found broken:

| v1 weakness | workbench answer |
|---|---|
| transcript inert | live stream, tool cards with state + expandable output, reasoning collapse, sys dividers (spawn / wake / drain / compact), hover-free density |
| no attention layer | bell + compact sheet (a *short* list, not a firehose) + red tints in the session list + `WAITING ON YOU` blocks in transcripts |
| sub-agents a static list | **first-class**: nested under the session in the left pane, live rows in the Crew dock, *and* live blocks in the parent transcript with follow-to-isolate |
| queue with no semantics | two labelled lanes with delivery timing ("next tool boundary" / "after work completes"), deletable |
| changes view took over the center | files in the dock + **non-modal peek** diff sheet; the center is always the transcript |
| thin navigation | ⌘K palette (20 cmds), ⌘. / ⌘\, ⌘1/2/3, j/k + Enter, F focus, r reasoning, Esc cascade |
| no liveness | 100 ms sim: streaming text, ticking tokens, tool states, sub-agent wakes/completions, OM gauge, cross-workspace needs |

## The 1 + 3–4 case, rendered

The differentiator is **crew legibility inside one conversation**:

- each sub-agent gets a bordered block in the parent transcript (state-coloured left edge,
  live tool line, tokens/elapsed, a `follow` button);
- `follow` dims every other entry (opacity .38) and pins a chip in the header — the
  worker's stream becomes *the* stream;
- completion lands a schema-validated **report card** in-thread (green, with the finding);
- the Crew dock mirrors all of it for at-a-glance; the left pane nests the crew under the
  session (spec §9: sub-agents nested, collapsible).

## Scale-up path (for the "some programmers have more" case)

Nothing in the home view changes as the fleet grows: the session list gets longer, the crew
list gets longer, the attention sheet gets a few more rows. The **fleet step-out** (⌘\) is a
plain list of all sessions across workspaces — a list absorbs scale that a four-column board
pretends to handle. Focus mode keeps the deep-work surface available at any scale.

## Bugs found in the visual pass

1. **Transcript never scrolled** — `.center` (and `.dock`) were grid items without
   `min-height: 0`, so they grew to full content height (3046 px) and the scroll container
   had nothing to scroll. The grid min-size trap, again.
2. **Peek sheet covered the topbar** — it was a child of `.body`, so its `position: absolute`
   resolved against the viewport, not `.center`. Moved inside `.center`.
3. **Sending to another session wiped the hero stream** — `send()` reset the *global*
   stream state (`streamChars/strDone`) which belonged to s1's scripted turn. It now only
   flips session state.
4. Un-sized inline SVGs (the follow-chip ✕ rendered huge) — sized explicitly.

(And the inherited one: the sim **freezes while any input is focused** — a background
re-render is never friends with a focused input.)

## Honest read

This is the prototype most likely to become the real v0 GUI, because it *is* the spec's
§9 layout — chat central, sessions left, tasks/sub-agents/files right, focus mode, steering
queue, lualine bar — rendered with the lessons from the other three. The board, feed, and
concierge survive as ideas for the step-outs: if parallel orchestration ever becomes common,
⌘\ is where the fleet goes, and nothing about the home view has to change.
