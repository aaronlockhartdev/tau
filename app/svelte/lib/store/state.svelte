<script module lang="ts">
  // The app's state bag (ticket #25) and the reads that need no round-trip.
  // $state lives in a .svelte module file because runes only transform there;
  // the feature modules (workspace, files pane, viewer, transcript, composer,
  // events) import this bag and mutate it, and `store.svelte` is the app-facing
  // facade that re-exports the whole surface (#320's split of the old monolith).
  //
  // The GUI is a stateless renderer (ADR-0006, live transport per
  // ADR-0008): every mutation is a command round-trip or an event
  // application, so a rebuild from snapshot + events always converges.
  // The transcript is an id-keyed map — entry_upsert carries the file's
  // own line, paged reads materialize the rest (spec §8).

  import {
    isTauri,
    type FileEntry,
    type GitStatus,
    type SkillInfo,
    type Workspace
  } from '../wire/protocol';
  import { presentation } from '../display/presentation.svelte';
  import { windowTitle as windowTitleOf, type SessionState } from '../rules/sessions';
  import { type FilesCache } from '../rules/files';
  import { defaultPane, paneOf, type PaneState } from '../rules/panes';
  import { type OpenFiles } from '../rules/openfiles';
  import { attentionOf, type AttentionItem } from '../display/attention';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  export const store = $state({
    focus: false,
    // The wall clock the relative-age labels read; ticked every 30s so idle
    // ages advance live (each tick rewrites only the label text).
    now: Date.now(),
    // Window fullscreen state: the top bar drops its traffic-light inset when
    // the OS hides the controls (tracked by the module-scope listener below).
    fullscreen: false,
    attentionOpen: false,
    paletteOpen: false,
    fleetOpen: false,
    focusInspector: null as 'left' | 'right' | null,
    // The follow filter (P2): the handle of the sub-agent the transcript is
    // focused on; non-followed entries render dimmed. Cleared by the chip.
    follow: null as string | null,
    modelMenuOpen: false,
    // Seeded from the per-user presentation registry (spec §12, #35):
    // 'visible-by-default' is the user's standing preference, and the 'r'
    // keybind toggles it for the session.
    reasoningOpen: presentation.reasoningVisibleByDefault,
    // Per-entry card expansion, keyed `${session}:${entryId}:${slot}`: the
    // transcript window unmounts off-screen cards, so the state lives here,
    // not in the card's local $state (which would reset to collapsed on
    // remount — scrolling to the bottom collapsed every expanded card).
    // A plain persistence map (not reactive — Svelte 5.57 deep-proxies
    // objects/arrays in $state, not Maps): EntryCard seeds its local
    // reactive state from it on mount and writes toggles back.
    entryOpen: new Map(),
    workspaces: [] as Workspace[],
    current: null as string | null,
    tailJump: 0,
    /* eslint-disable @typescript-eslint/no-unnecessary-type-assertion -- the $state generic accepts the bare literal; the casts pin the inferred type ({} → Record<...>) */
    sessions: {} as Record<string, SessionState>,
    loading: false,
    error: null as string | null,
    // The transcript's last window computation (spec §9 seg3 render stats).
    renderRange: '',
    // The skill registry, cached per workspace (ticket #28): the
    // composer's /skill: autocomplete data source.
    skills: {} as Record<string, SkillInfo[]>,
    // The files pane (ticket #32), per workspace: the listed dirs and
    // their entries. A dir appears here once fetched (workspace open
    // lists the root, expansion lists its children); FileTreeChanged
    // marks a listed dir stale and a coalesced refetch wave replaces it.
    files: {} as FilesCache,
    // Per-dir file_list failures (the empty-directory deception): a failed
    // listing is recorded per workspace/path and cleared when the same dir
    // lists successfully again.
    fileErrors: {} as Record<string, Record<string, string>>,
    // Git status per workspace (workbench P4): the tree's change tints and
    // the changes view's file switcher. null = not a git repository.
    gitStatus: {} as Record<string, GitStatus | null>,
    // The file viewer (open-files model): a persistent, per-workspace set of
    // open files (closable tabs), independent of git status. The center column
    // toggles between the transcript and the viewer (pane.center); open files
    // survive a switch to the transcript. activeFile* is the shown tab: a null
    // New is "loading", a set Error is a real reason (binary / unreadable), ''
    // is an empty file.
    openFiles: {} as OpenFiles,
    activeFile: null as { workspace: string; path: string } | null,
    activeFileNew: null as string | null,
    activeFileBase: null as string | null,
    activeFileError: null as string | null,
    // Per-tab isolation (spec §9): each open workspace owns its pane view
    // state; the transcript's conversation state stays per-session.
    pane: {} as Record<string, PaneState>,
    /* eslint-enable @typescript-eslint/no-unnecessary-type-assertion */
    // The tab strip's multiselect (B2): the pane's session-select shape
    // (PaneState's selected/selAnchor), but global — a selection can span
    // tabs of several workspaces, so it keys on nothing.
    tabSelected: [] as string[],
    tabSelAnchor: null as string | null
  });

  // Advance the wall clock on a slow tick so relative-age labels (session last_modified,
  // attention "since") stay current without session activity.
  setInterval(() => {
    store.now = Date.now();
  }, 30_000);

  // The top bar reserves left padding for the macOS traffic lights; they're
  // hidden in fullscreen, so we drop the inset there. Tauri has no
  // fullscreen-changed event, so we track it off the window resize event
  // (tauri-apps/tauri#7162). Entering is clean: isFullscreen() flips true at
  // the animation's start. Exiting is the catch: isFullscreen() stays true
  // until the animation ENDS, by then the lights are back and briefly cover
  // the logo. The resize event, though, fires at the exit animation's START,
  // and a resize while settled in fullscreen can only be that exit (macOS
  // won't let you resize a fullscreen window) — so we restore the inset then,
  // before isFullscreen catches up. The grace window skips the enter
  // animation's own resizes; `exiting` ignores the stale-true mid-exit.
  // A rapid exit inside the grace window is indistinguishable from the enter
  // animation (no pre-transition signal), so it keeps the inset dropped once — cosmetic.
  if (isTauri()) {
    const win = getCurrentWindow();
    let enterAt = 0;
    let exiting = false;
    let exitAt = 0;
    const settle = 1200; // longer than the enter animation, so its resizes fall inside the window
    const sync = () =>
      void win.isFullscreen().then((fs) => {
        if (exiting && fs && Date.now() - exitAt < settle) return; // stale-true only mid-exit
        exiting = false;
        store.fullscreen = fs;
        enterAt = fs ? Date.now() : 0;
      });
    sync();
    void win.onResized(() => {
      if (store.fullscreen && Date.now() - enterAt > settle) {
        exiting = true;
        exitAt = Date.now();
        store.fullscreen = false; // exit animation starting; move the logo clear now
        return;
      }
      sync();
    });
  }

  // The needs-you list (P5): a pure fold over the sessions, so any pane
  // (bell, sheet, tree tint) reads one shared projection. A function —
  // derived state may not be exported from a module.
  export function attention(): AttentionItem[] {
    return attentionOf(store.sessions);
  }

  // Pure read — a $derived may call this; creation goes through ensurePane
  // (a mutation, only from effects/actions).
  export function pane(ws: string | null): PaneState | null {
    return paneOf(store.pane, ws);
  }

  // Create-if-missing (the old pane() behavior) — call from effects/actions.
  export function ensurePane(ws: string): PaneState {
    const prev = store.pane[ws];
    if (prev) return prev;
    const p = defaultPane();
    store.pane[ws] = p;
    return p;
  }

  // Read facade (F2): the current-session and per-workspace pane / file-cache
  // reads the views each re-derived from the raw bag with their own null
  // discipline, centralized so the bag is an implementation detail. Read-only.

  export function currentSession(): SessionState | null {
    const c = store.current;
    return c ? store.sessions[c] ?? null : null;
  }

  export function fileCache(ws: string): Record<string, FileEntry[]> | undefined {
    return store.files[ws];
  }

  export function toggleAllReasoning(): void {
    store.reasoningOpen = !store.reasoningOpen;
  }

  export function setFollow(handle: string | null): void {
    store.follow = handle;
  }

  export function windowTitle(): string {
    return windowTitleOf({
      current: store.current,
      sessions: store.sessions,
      workspaces: store.workspaces
    });
  }

  export function sessionOf(sid: string): SessionState {
    const s = store.sessions[sid];
    if (!s) throw new Error(`unknown session ${sid}`);
    return s;
  }

  // Banner lifecycle (dogfood N2): every command clears the stale banner
  // before it runs, a failed command sets a fresh one, and non-error
  // system events no longer wipe it (applyEvents).
  export function clearError(): void {
    store.error = null;
  }
</script>
