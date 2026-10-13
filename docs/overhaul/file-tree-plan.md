# File tree plan — icons, change stats, and the changes tab

2026-10-12. Follow-on to the workbench rebuild (`workbench-gui-plan.md`). Closes the
remaining half of issue #116 ("full-featured file tree") — the diff-viewer half shipped
in workbench P4 (the peek sheet); this plan is the file-tree half: type icons, git
change stats in the tree, highlighting of modified files, greying of .gitignored
files, and a changes tab.

Requested shape (2026-10-12, user): VS Code-*like* iconography (similar icons, not a
clone of that theme), `+N/−M` stats in the tree, modified files highlighted, ignored
files **shown but greyed out** (the display set stays the filesystem walk — ignore
status only changes styling).

## Current state (what we build on)

- `FileEntry { name, path, dir, size }` (`crates/tau-protocol/src/lib.rs`);
  `list_dir` (`crates/tau-core/src/harness/watchers.rs`) is a plain `read_dir` —
  the tree shows everything on disk, ignore- and change-blind.
- `GitStatus { branch, modified, untracked, deleted }` via `git status --porcelain=v2`
  (`harness/git.rs`, with parser unit tests + isolated integration tests); the store
  caches it per workspace and refreshes on workspace open and `file_tree_changed`.
- `FileNode.svelte` renders `name` + a letter badge (D > A > M); file rows open the
  peek sheet. `LeftPane` is segmented `files | sessions`.
- Peek sheet + `git_diff` (P4) exist — the changes tab reuses them for drill-down.

## Protocol decisions

One extension, additive:

```rust
pub struct FileEntry {
    name, path, dir, size,        // unchanged
    pub ignored: bool,            // in .gitignore (shown, greyed)
    pub git: Option<FileGit>,     // None outside a git workspace
}
pub struct FileGit {
    pub state: FileGitState,      // Modified | Untracked | Deleted
    pub added: Option<u32>,       // numstat; None when unmeasurable
    pub deleted: Option<u32>,
}
```

- No new command for the tree: the `file_list` arm merges a per-workspace git
  snapshot into `list_dir`'s output. One new command for the changes tab's stats:
  `git_numstat { workspace }` → `Vec<NumstatEntry { path, added, deleted, state }>`
  (`git diff --numstat` + the status letter; untracked files get their line count
  as `added`, `deleted: 0`).
- Ignored set: `git ls-files --others` minus `git ls-files --others
  --exclude-standard` — set difference, no per-file stat.
- TS mirror + `protocol.test` round-trip, same pattern as the P4 git commands.

## Phases

### Phase 1 — File type icons (GUI only)

Extend the App.svelte sprite with a curated set in the existing stroke style:
folder, folder-open, and ~12 file kinds (ts, js, json, rs, toml, md, svelte, css,
html, image, lock, git-config) + generic file. `fileIcon(name): string` in
`lib/files.ts` (extension → symbol id; dirs get folder icons; unknown → generic).

DoD: every tree row shows its icon (14px, tinted per kind where sensible);
`fileIcon` unit tests positive (`.ts`, `.rs`, `Cargo.toml`) and negative
(`.xyz` → generic). No protocol change.

### Phase 2 — Git-aware tree (protocol + core + tree styling)

- Core: `git.rs` gains `git_numstat` + the ignored-set derivation; a
  `git::snapshot(cwd)` combines status/numstat/ignored (three spawns), cached
  per workspace and invalidated on `file_tree_changed` (which already
  refetches). `list_dir`'s output is annotated with `ignored` + `git`.
  Non-git workspaces: `ignored: false`, `git: None` — today's tree unchanged.
- GUI: tree rows render icon + **tinted name** (modified → amber, untracked →
  green, deleted → red) + `+N −M` in green/red at the row's right edge; the
  letter badge retires (tint + stats replace it). Ignored rows: `opacity: 0.45`,
  grey icon, no tint/stats.

DoD: in a git workspace the tree shows stats/tints; `.gitignore`d paths render
greyed but present; a `target/`-sized ignore set costs one cached snapshot, not
per-row work. Parser tests for numstat + ignored-diff (isolated git fixtures,
same pattern as `tests_git.rs`); FileNode component tests for tint, stats, and
grey rows.

### Phase 3 — Changes tab (GUI only)

LeftPane becomes `files | changes | sessions` (the segmented control already
handles three tabs). The changes tab lists `modified + untracked + deleted`
from the existing `gitStatus` + the new numstat, grouped by directory, rows
reusing the Phase-2 row styling (icon, tinted name, `+N −M`); a count badge on
the tab; row click opens the peek sheet (`setPeek` — P4 path, unchanged).
Empty state: "no changes".

DoD: tab shows the same set the tree tints; click-through to diffs works for
all three states; LeftPane test for the tab, its rows, and the peek wiring.

## Risks

- **Git spawn cost on expand:** every `file_list` would trigger the snapshot.
  Mitigated by the per-workspace cache keyed to the `file_tree_changed`
  invalidation the store already throttles; the snapshot is three spawns total,
  shared by all dir listings in the workspace.
- **numstat on untracked files** is not in `git diff --numstat`; the line-count
  fallback must be a pure, tested function (and capped — a 10k-line untracked
  file is just a big number, no special handling).
- **Icon set sprawl:** the set is curated to what this repo and typical tau
  workspaces show; new kinds are a one-line mapping + one symbol, not a
  re-architecture. No icon-font dependency (sprite constraint, no-emoji rule).

## Tickets

Covers and closes #116 on completion. Phases land as: (1) icons commit,
(2) protocol+core+styling commit, (3) changes-tab commit — each gate-green
(svelte-check, vitest, eslint; core: nextest + clippy).
