# Prototype board v2 — critique, research, and design

Research for the second prototype pass over `app/prototype-board.html` (the
"precision" list-detail skin). Companion to `agent-gui-layouts.md`
(2026-10-11), which settled pane topology; this pass covers the **ergonomics
layer**: attention, steering, transcript affordances, keyboard model, cost
legibility — and the **board** concept the v1 file promises but never ships.

All new sources accessed **2026-10-12**.

## 1. What v1 gets right (keep)

- Sessions-first left list, transcript center, right dock — the industry's
  strongest convergence (`agent-gui-layouts.md` synthesis §1–3).
- The "precision" skin: compact 13px density, tokens, dark palette — a
  credible production look.
- Focus mode with edge pill rails — a genuine differentiator (no product
  documents a two-rail focus mode; `agent-gui-layouts.md` "implications").
- Command palette skeleton, queue above composer, lane pills, compaction
  divider, per-session model display.

## 2. Critique of `prototype-board.html`

### A. The board is missing

1. **The filename promises a board; there is none.** The center is a single
   chat/changes surface. There is no fleet view, no at-a-glance state of the
   N parallel sessions the app exists to run. The one feature that justifies
   the "board" name is absent.
2. **Left session list is a flat list.** No sub-agent grouping under parents
   (spec §9: nested collapsible groups), no archive folder, no search, no
   lifecycle tags on child rows (spec: sub-agent rows keep full status tags),
   no per-session state beyond one 6px dot.
3. **No "which need me" signal anywhere.** The core productivity gap of the
   whole category (see §3.1) is unmodeled.

### B. Attention / notification layer (entirely absent)

4. **No needs-input state.** Spec §5.2: `idle, waiting_on: user` → "GUI
   badge only". Nothing in v1 models a session that is parked waiting for the
   user.
5. **No notifications** (in-app, OS, sound) for: turn ended, sub-agent
   done/failed, task blocked, model asked a question.
6. **No attention aggregator** (bell / activity view) across workspaces.

### C. Transcript

7. **Tool rows are inert.** No expand to args/output (the protocol carries
   `ToolPayload.output`), no error state, no group collapse.
8. **Reasoning is a static line.** Spec §6: viewable *and collapsible* with a
   visible-by-default setting (the real app has `r` + per-block toggles).
9. **No entry hover actions.** No copy, no *branch from here*, no *fork from
   here* (OpenHands ships branch-from-any-message; branching in place is a
   headline feature with zero UI affordance in v1).
10. **Compaction divider is inert.** No way to inspect the compacted span —
    the `recall` model deserves a surface.
11. **No per-turn cost** in entry headers (`stream_end` carries usage), no
    timestamps on user entries.
12. **No branch/leaf indicator** in the center (a session is a tree).
13. **No windowing affordance** for the 10k-entry stress story (spec §8 perf
    bar): no "N entries above", no jump top/bottom, no render window in the
    bar.

### D. Queue and composer

14. **Queue rows: no delete** (the real app has it), no restore-to-editor, no
    three-section semantics (sub-agent reports / next opportunity / after
    work completes), no preview of what the model will see.
15. **Lane selector is three bare words.** Force *kills the in-flight
    stream* — a destructive act that should read as distinct (color,
    semantics in a tooltip), not a peer of the others.
16. **No stop affordance at all.** `message_stop` is in the protocol; the
    real composer morphs send→stop. In v1 a running session cannot be
    stopped from the UI.
17. **No `/` autocomplete.** The real app has `/model`, `/help`,
    `/skill:…`; `SkillInfo` is documented as "the composer autocomplete's
    data source" — absent in v1.
18. **No `@` file mentions** (novel; `file_list` makes it feasible).
19. **Model chip is inert** — no menu (`ProviderInfo` in the protocol is
    already "one provider entry as the GUI sees it" — the menu's grouping
    source).

### E. Right dock

20. **Tasks:** no status filter (spec: filterable, default not-done); missing
    sections (acceptance criteria, resume contract, decisions ledger); no
    actions (cancel/assign); no worker-pointer click-through (it is in the
    protocol).
21. **Sub-agents:** flat rows, no tree (spec: tree, nesting supported); no
    context-mode chip (`fresh/compacted/fork` is a headline protocol concept);
    no actions (open session / stop / message-resume); no usage.
22. **Files in the right dock contradicts spec §9** (left pane is
    `files | sessions`) — and the tree has no git/diff badges; clicking a
    file does nothing.

### F. Changes view

23. **Full-center takeover.** Inspecting a diff forces abandoning the chat —
    no peek/inspector alternative, no entry from a file-tree click.
24. **No ship actions.** No commit flow, no unified/split toggle, no per-file
    actions. The board category's endgame is *review → commit/PR from the
    app* (Conductor, Vibe Kanban).
25. **No changed-file linkage** between the file tree and the changes view.

### G. Navigation

26. **Palette: 8 commands**, no session jump, no file open, no fuzzy match,
    no keymap page. ⌘K is the universal door in every full-surface product.
27. **No list navigation keys** (⌘1–9 / ⌘⇧[ ] / ⌥↑↓); Antigravity ships
    ⌥Up/Down conversation nav — a cheap, high-value binding.
28. **Focus toggle is an unlabeled icon** — a discoverability hole for the
    headline differentiator.
29. **Focus sheets are scrim-modal.** In focus mode the user wants a
    *non-modal* inspector coexisting with the transcript (OpenHands' drawer
    pattern), not an overlay that blocks the view.

### H. Status bar, chrome, robustness

30. **Status bar missing:** current lane, queue depth, streams count, render
    window/ms, git branch — the spec §9 lualine shape is
    `[ws · session · state] [lane · queue] [om · render · streams · model]`.
31. **Workspace tabs:** close/multiselect/bulk behavior absent (the real app
    has it), no cwd.
32. **No empty states** (no-workspace boot was a live bug — `gui-bugs.md` #6).
33. **Accessibility:** no `:focus-visible` styles, icon buttons without
    labels, color-only status, no reduced-motion.
34. **No liveness.** A static snapshot of an *agent harness* — a prototype
    whose subject is live parallel work should stream, change state, and
    notify, so the async mental model is felt, not described.

## 3. New research (2026-10-12)

### 3.1 The "board" is now its own product category

The v1 file's name points at a category that has hardened since the layout
doc was written:

- **Conductor** (conductor.build) — the "agentic parallel runner" pioneer:
  one worktree workspace per feature; the dashboard "shows what every agent
  is doing **and where it's stuck**"; diff viewer on ⌘⇧D; a **Checks tab**
  (git status, CI, review comments, todos); *draft a PR from the current
  diff*; archive finished workspaces "so your sidebar stays focused".
- **Vibe Kanban** (BloopAI; now community-maintained) — kanban issues →
  workspaces; every card an isolated worktree + agent; card status + latest
  agent output; visual diff review; "Open in IDE".
- **Crystal → Nimbalyst** — parallel Codex/Claude sessions in worktrees;
  "test, **compare approaches**".
- **Omnara / Happy Coder** — the remote angle: steer from phone/watch;
  "sessions that never die when your laptop goes offline".

**Synthesis:** the board's job is *fleet status* (what each is doing, where
stuck, **what needs me**) + *diff review* + *ship*. The atom is a card with
status + latest output line (Cline Kanban's pattern, adopted by Conductor).
Copilot's Mission Control is the same idea inside a platform: "jump in when
input is needed".

**Implication for tau:** ship the board as a *view inside the harness*
(center switch: **Board | Chat | Changes**) — no separate app, no worktree
requirement in v0 (worktrees are a v1+ isolation story). The board is a
projection of the sessions the harness already runs; that is the
differentiator vs. Conductor/Vibe Kanban, which wrap external CLIs.

### 3.2 The attention layer (industry pattern)

- **Codex cloud:** an **Activity view** (bell) "brings together chats you
  recently engaged with and work that needs your attention"; sidebar states
  include *requires-input*.
- **Devin:** notifications (Slack/Teams/email) "when a session **needs
  input**"; sidebar carries status + PR state.
- **Factory:** notifications for completion *and* requested input;
  **per-subagent sounds**; a "sound focus mode".
- **Copilot:** mission control "jump-in when input needed"; token usage per
  session in the agents panel.

**Synthesis:** "needs you" is a *computed* state — question at turn end,
child `idle waiting_on: user`, task `blocked`, `failed` — aggregated into a
bell + list badges + toasts/OS notifications + optional sound. **No protocol
change is required**: every ingredient already arrives as events
(`stream_end` with a question-shaped final message, `subagent_event`,
`task_changed`). It is a GUI-side fold.

### 3.3 Ergonomics baseline: pi's keymap

Tau's users are pi users; the GUI should respect that vocabulary (GUI-ified):

| pi TUI | meaning |
|---|---|
| `escape` | **interrupt the in-flight turn** (`app.interrupt`) |
| `alt+enter` | queue follow-up message |
| `alt+up` | **restore queued messages to editor** |
| `ctrl+l` | model selector |
| `shift+tab` | cycle thinking level |
| `ctrl+t` | toggle thinking blocks |
| `ctrl+o` | expand/collapse tool output |
| `ctrl+p / s / n / r / d` | path / sort / named filter / rename / delete (session list) |

The palette's **keymap page** is where the GUI mapping is documented.

### 3.4 Micro-interactions worth stealing

- **OpenHands:** a **live activity chip** in the composer ("running a
  command" / "reading a file" / "Thinking"); a **context-window meter in the
  composer** with manual compaction; *branch from here* on any message;
  retry/dismiss on failed bubbles.
- **Zed:** per-queued-message **Steer toggle + "Send Now"**; **checkpoint
  restore** at any message that edited files.
- **Cursor:** checkpoints in the chat timeline (preview/restore codebase
  state at that point).
- **Conductor:** archive finished work to keep the board focused.

## 4. Design principles for v2

1. **The board is a first-class center view**, not an app of its own.
2. **Attention is a computed, global, aggregated state** — bell, badges,
   toasts; the "Needs you" column is the board's red channel.
3. **The transcript is a work surface, not a log**: every entry has actions
   (copy / branch / fork), tools expand, reasoning collapses, compaction is
   inspectable, turns carry cost.
4. **Steering is loud where it is destructive**: force reads red and explains
   itself; stop is always reachable in the composer; the queue is editable.
5. **The keyboard is a second interface**: ⌘K universal door + keymap page;
   pi's vocabulary ported to super keys; `esc` interrupts (pi parity).
6. **Everything legible is legible**: lualine bar, per-turn cost, OM gauge
   with pending, streams, render window.
7. **Focus mode = non-modal inspectors + ⌥↑/↓ session hopping.**
8. **The prototype is live**: it streams, its fleet changes state, and
   attention arrives — the async mental model is demonstrated, not described.

## 5. Novel ideas (beyond the field)

1. **`/agent:<type> <brief>` one-liner delegation** in the composer — the
   protocol already has `subagent_spawn {agent_type, brief}`; a slash
   shortcut makes *delegation a sentence* (research: nothing in the field
   offers spawn-from-composer; spawning is a model tool call or a pane form).
2. **`@path` file mentions** in the composer (fuzzy over `file_list`) —
   pin context into a message without a tool round-trip.
3. **Turn timeline strip** — a segmented bar of the *current turn*
   (thinking → tool chips with elapsed) above the transcript; click a chip
   to jump. "What is it doing right now" without scrolling. The field shows
   activity chips (OpenHands) or step feeds (Jules) — a per-turn *timeline*
   is unclaimed.
4. **Compaction recall cards** — the compaction divider expands into the
   compacted span's metadata (count, first kept, previews) — makes OM
   legible; no product surfaces its memory mechanism in the transcript.
5. **Resume-contract cards in the transcript** — when a compaction rides on a
   session with an active task, render the re-injected resume contract as a
   card (spec §5.3 already re-injects it; the GUI never shows it).
6. **Board summary strip with fleet cost** — running/needs-you/idle/done
   counts + total tokens + estimated $, live.
7. **Sound focus mode** (Factory pattern) — per-event sounds (sub-agent
   done, needs-you) behind a settings toggle; the focus-mode companion.
8. **Non-modal inspector in focus mode** + ⌥↑/↓ hopping — the focus mode
   becomes a "one window, many agents" cockpit, not just a wider chat.
9. **Needs-you reason chips** — every red state carries *why* ("asked a
   question", "sub-agent waiting on you", "blocked: needs X"). The field
   shows states; reasons are the differentiator.
10. **Queue "what the model will see" preview** — hover a queued item to see
    its delivery semantics (next batch / after work / head-of-queue for
    force), making the three lanes *understandable*, not just selectable.

## 6. Protocol gaps the prototype exposes

- **No manual compaction command.** The composer's context meter wants a
  `session_compact` (or an explicit trigger for the Observer); today only
  thresholds fire it.
- **No git state in the protocol.** File-tree diff badges and the status
  bar's branch segment need a `git_status`-class source (post-v0, or
  agent-read via `bash`).
- **No notification primitive.** Not needed for v0 attention (derived,
  §3.2) — but OS notifications for *unfocused* needs-you will want one.

## 7. What `prototype-board-v2.html` demonstrates

- Center view switch **Board | Chat | Changes** (⌘1/2/3) with live fleet
  cards (Running / **Needs you** / Idle / Done), summary strip, card → chat.
- Attention: global bell + count, needs-you sheet with jump, red dots in the
  session tree, toasts; a simulated cross-workspace needs-you event.
- Transcript: turn timeline, expandable tools, collapsible reasoning, hover
  actions (copy/branch/fork), recall cards, spawn cards, per-turn cost,
  branch chip.
- Queue: three sections, delete, restore-to-editor; composer: force/steer/
  follow lanes (force red, self-explaining), stop morph, `/` and `@`
  autocomplete, model menu.
- Left pane **Sessions | Files** per spec §9: nested sub-agent groups,
  lifecycle tags, search, archive folder; git-badged file tree with
  changed-file → diff jump.
- Right pane **Tasks | Sub-agents** per spec: filters, full task sections
  (steps + expected output, criteria, evidence, blockers, resume contract,
  worker click-through); sub-agent tree with context-mode chips and
  open/stop/message actions. The task card's *nudge worker* / *branch from
  task* buttons and the sub-agent *message* prefill are exploratory
  post-v0 affordances with no v0 protocol counterpart (the one-shot nudge
  is core-internal, §5.2; sub-agent messaging is `subagent_message` to the
  child's own lane, §5.1).
- Changes: stat bar, unified diff, per-file list, commit row (v0-honest:
  "ask the agent"), file-tree linkage.
- Focus mode: pill rails, **non-modal** inspectors, ⌥↑/↓ session hopping.
- Palette: ~20 commands, session jumps, **keymap page** documenting the
  pi-parity bindings.
- Lualine status bar: state · ws · session · lane · queue | om + pending ·
  render window · streams · model · tps.
- A live ~22-second scripted simulation: stream growth, turn end, queue
  drain, sub-agent completion, cross-workspace attention event.

## 8. Build status

`app/prototype-board-v2.html` (single file, no dependencies, 1,673 lines) is
built and verified in Chrome. Every user navigation path was walked with
screenshots: initial chat load, board → card → chat, changes view with
file-list and tree → diff jumps, files tab with git badges, sub-agent
tabs, the bell → cross-workspace needs-you → reply loop, focus mode with
edge rails and non-modal inspector sheets, command palette with keymap
page, `/` and `@` autocomplete, and the model menu. The walk surfaced and
fixed three bugs (the app root was not height-constrained so the status
bar and composer could clip off-screen; focus mode rendered an empty
void because the `.focus` class was never applied; the palette input lost
focus when opened from the button) plus two composer cosmetics (lane
label wrap, OM-meter truncation). Open it directly in a browser — no
server needed.
