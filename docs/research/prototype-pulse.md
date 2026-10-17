# τ pulse — feed-first prototype

`app/prototype-pulse.html` (single file, no dependencies, ~523 lines). A
second, deliberately different take on the agent-harness GUI. Open it
directly in a browser.

## The inversion

The board v2 prototype (`prototype-board-v2.html`) uses the industry-standard
**list-detail** pattern the 11-product survey found everywhere: *pick a
session, then look*. Pulse inverts the primary axis from **session** to
**time**.

- **Home is the stream, not the list.** One continuous, coalesced feed of
  every event across all workspaces — turns, tool calls, file writes,
  sub-agent lifecycle, cost, compaction, attention. The answer to
  "what's happening?" is always on screen with zero navigation.
- **The system comes to you.** Attention is structural, not a modal: a
  pinned *needs you* section sits at the top of the feed, and a live strip
  of running-session tiles keeps fleet status persistent. Nothing needs you
  can be buried under a session you have to open.
- **A session is a docked drawer, not a view.** Clicking any row (or a rail
  dot, or a palette command) slides a session drawer in from the right with
  its recent transcript, steering/follow-up queue, and a composer. You can
  keep scrolling the feed while the drawer stays open. There is no separate
  "chat view" — the feed is home.

The through-line: **minimize the distance between a signal and your
intervention.** Board v2 optimizes for *scanning the fleet*; pulse optimizes
for *reacting to the fleet*.

## What it has

- Coalesced feed (one row per turn/tool/file/sub/attention/cost/compaction),
  live-follow with a jump-to-latest pill, per-workspace + per-kind filters,
  and search.
- Pinned *needs you* section + live strip of running sessions.
- Session drawer: recent transcript, queue, lane-aware composer (send posts
  into the feed and flips the session to running).
- Slim left rail: workspace dots + session dots (running ones pulse).
- Command palette (⌘K), `j/k` row navigation + `⏎` to open, `/` search,
  `1–7` filters, `l` live toggle, `d`/`esc` close.
- A ~24 s scripted simulation interleaves three running sessions so the
  feed visibly streams, a needs-you event fires, and turns complete.

## Known rough edges (prototype scope)

- The drawer transcript is derived from feed events (last 16), not a full
  session tree — enough to show the pattern, not a real transcript.
- `j/k` selection can land behind the sticky *needs you* header.
- No board/changes view here by design — that's v2's job. The two are
  complementary, not competing: pulse for "what needs me right now", board
  for "what's the state of everything".
