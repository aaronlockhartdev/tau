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
- **Side panes render no tabs at all until a session exists** — the
  pane headers disappear with the empty state; the demo never hits this
  path.
- **Console-error monitoring unverified** — the display locked mid-run
  and the console-monitor log came back empty; the live journey was
  verified through the DOM, session files, and screenshots instead.
