# GUI live-journey findings (2026-09-19)

Explored with tauri-agent-tools driving the real development app (bridge
wired in `app/src-tauri/src/dev_bridge.rs`, debug builds only). The demo
mode (10k fixture) verified clean across panes, streams, queue, badges,
archive, and double-click-to-open; the live user journey surfaced the
bugs below.

## Fixed

1. **No Tauri capabilities file** — `event.listen` was denied, so the
   protocol stream (the only live path) never reached the store. Added
   `app/src-tauri/capabilities/default.json`
   (`core:event:allow-listen`, `core:event:allow-unlisten`,
   `dialog:allow-open`).
2. **`CommandOutput` list variants unserializable** — the six newtype
   `Vec` variants cannot serialize under the internally tagged enum;
   every list command failed at the wire. Converted to struct variants
   with the TS contract field names; round-trip regression test added.
3. **`Session` / `Snapshot` / `Workspace` newtype variants flattened on
   the wire** — an internally tagged newtype merges the tag into the
   payload's map, so the wire carried the payload fields at the top
   level while protocol.ts reads the named field. The store crashed with
   `undefined is not an object (evaluating 'snap.session')`, silently
   killing session open (no transcript, no composer, no panes).
   Converted to struct variants; test patterns updated.
4. **Store dropped `workspace_opened` system events** — `applyEvents`
   short-circuited any event without a `session`, which is exactly the
   shape of `workspace_opened`; a workspace opened by any client other
   than the store's own `openWorkspace` was invisible to the GUI. The
   store now re-reads the list on the event and opens the first
   workspace when nothing is open (the boot rule, applied live).
5. **Core tools broken in every live top-level session** — the agent
   loop routed by link presence (`if let Some(sup) = …`), so any session
   carrying a supervisor sent *every* tool call into `route_parent`,
   which only knows `subagent_*` names: `bash` came back
   `unknown tool: bash` (seen live; the model retried and gave up).
   Children had the mirror bug for `parent_notify`. Routing is now by
   tool name: `subagent_*` → supervisor, `parent_notify` → child link,
   everything else → the core dispatch.
6. **No live affordance to open a workspace** — the + menu was
   demo-only, so a machine with no sessions was stuck at "no workspace
   open". The live + item now opens a folder picker
   (tauri-plugin-dialog) and the picked directory goes through
   `workspace_open`.
7. **`openWorkspace` swallowed command failures** — e.g. "no providers
   configured" produced a dead empty session with no error surface.
   The open path now reports to the store's error bar.
8. **`__tau` verification seam was demo-only** — made mode-agnostic
   (`applyEvents` + null-safe `liveTexts`) so the live wire path is
   testable from the page.
9. **`Subagent` / `File` newtype variants + a two-variant TS mirror gap**
   — same flattening class as #3 (the `file` member declared nested
   while the wire was flat), and the TS `CommandOutput` union was
   missing `subagent` and `subagents` entirely. Converted both Rust
   variants to struct variants and added the two TS members, so the
   mirror is field-for-field on all twelve variants.
10. **`openWorkspace` double-open race** — the open's own
    `workspace_opened` event re-entered the store via `syncWorkspaces`
    mid-open (no in-flight guard); on an empty workspace the two
    `session_new` calls created a phantom duplicate session. An
    in-flight marker now guards both entry points.
11. **Side-pane tab headers vanished in the no-workspace empty state** —
    the pane bodies (and their tab rows) were guarded by `{#if p}`; the
    tab rows now render disabled outside the guard.
12. **Live transcript corruption on hydration** — snapshot entries and
    streamed entries live in two id namespaces (file counter vs
    call_id) that the merge compared as if one ordered space: a paged
    read hydrating the assistant's file copy mid-turn produced a
    duplicate against the streamed copy at turn end, and a second
    turn's file entries inserted before the first turn's answer
    (reviewer-confirmed, rig at the two-turn session). The hydration
    merge now drops a file entry that matches a streamed twin (by text
    for message/user, by the shared call_id for tools — in entries or
    live) and appends twin-less entries in arrival order; the user
    bubble appears at send time instead of only after the turn.
13. **Live display gaps (six, user-reported)** — tool calls invisible
    after a turn (payloads never hydrated), reasoning title floating
    away from its block with a cursor that stayed on after completion,
    a model that only saw 5 of the 16 v0 tools, an idle send queuing
    instead of starting, and no TPS / prefix-cache segments in the bar.
    All fixed in 698e691; verified live and by the two-turn rig.

## Environmental (not code)

- `~/.config/tau/config.toml` did not exist, so `session_new` failed
  with "no providers configured". Resolved by copying `dev/config.toml`
  (the documented dev setup). The core reads config at construction, so
  a config change needs an app restart (no live reload in v0 —
  expected).

## Open (recorded, not fixed)

- **Workspaces do not survive an app restart** — the workspace map is
  in-memory and boot never reconstructs workspaces from the on-disk
  sessions (`{project}/.tau/sessions/`), so a restart puts the user
  back at "no workspace open" with the + button.
  path.
- **Console-error monitoring unverified** — the display locked mid-run
  and the console-monitor log came back empty; the live journey was
  verified through the DOM, session files, and screenshots instead.
