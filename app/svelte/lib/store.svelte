<script module lang="ts">
  // The app-facing facade for the central column (ticket #25): the state bag
  // lives in state.svelte, the round-trips in the feature modules
  // (workspace, files-pane, viewer, transcript, composer, events), and this
  // file re-exports them as one surface (#320's split of the monolith).
  //
  // The GUI is a stateless renderer (ADR-0006, live transport per
  // ADR-0008): every mutation is a command round-trip or an event
  // application, so a rebuild from snapshot + events always converges.
  // The transcript is an id-keyed map — entry_upsert carries the file's
  // own line, paged reads materialize the rest (spec §8).

  import { command } from './protocol';
  import {
    archiveSession,
    deleteSession,
    newSession,
    refetchSessionList,
    renameSession,
    restoreSession,
    runSessionCommand,
    switchSession
  } from './commands';
  import {
    attention,
    currentSession,
    ensurePane,
    fileCache,
    pane,
    setFollow,
    store,
    toggleAllReasoning,
    windowTitle
  } from './state.svelte';
  import { closeWorkspace, closeWorkspaces, init, openWorkspace, setModel, tabSelect } from './workspace.svelte';
  import { retryDirFetch, toggleFileDir } from './files-pane.svelte';
  import { closeFile, openFile, reconcileActiveFile, selectFile, setCenterMode } from './viewer.svelte';
  import { fetchWindow } from './transcript.svelte';
  import { deleteQueueItem, send, stop } from './composer.svelte';
  import { applyEvents } from './events.svelte';

  export {
    applyEvents,
    archiveSession,
    attention,
    closeFile,
    closeWorkspace,
    closeWorkspaces,
    currentSession,
    deleteQueueItem,
    deleteSession,
    ensurePane,
    fetchWindow,
    fileCache,
    init,
    newSession,
    openFile,
    openWorkspace,
    pane,
    reconcileActiveFile,
    refetchSessionList,
    renameSession,
    restoreSession,
    retryDirFetch,
    runSessionCommand,
    selectFile,
    send,
    setCenterMode,
    setFollow,
    setModel,
    stop,
    store,
    switchSession,
    tabSelect,
    toggleAllReasoning,
    toggleFileDir,
    windowTitle
  };
  export type { PendingMsg, SessionState } from './sessions';
  export type { PaneState } from './panes';

  // Verification seam (the dev rig): feed wire-shaped events through
  // applyEvents and read the current session's live-stream lengths (store level — the
  // virtualized DOM only shows the window, one stream card can sit outside it).
  (window as unknown as { __tau?: unknown }).__tau = {
    applyEvents,
    store: () => store,
    // send/stop/openWorkspace/closeWorkspace/switchSession/fetchWindow
    // are module exports the rig drives directly; hoisted above.
    send,
    stop,
    openWorkspace,
    closeWorkspace,
    switchSession,
    fetchWindow,
    omStatus: (kind: 'observing' | 'reflecting' | 'idle') =>
      applyEvents([
        {
          type: 'om_status',
          workspace: store.current ? store.sessions[store.current]!.meta.workspace : '',
          session: store.current ?? '',
          kind
        }
      ]),
    sessionSetModel: (session: string, model: string) => command({ type: 'session_set_model', session, model }),
    snapshot: (session: string) => command({ type: 'session_snapshot', session })
  };
</script>
