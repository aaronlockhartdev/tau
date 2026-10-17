# Migrating the file-tree git harness from `git` shell-outs to `gix`

**Status**: done (2026-10-13). **Recorded as**: ADR-0011. **Supersedes**: the shell-out
implementation in `crates/tau-core/src/harness/git.rs`.

## Why

The file-tree git awareness (file-tree plan, phases 2–3) shells out to the `git`
CLI. Five spawns per snapshot (`status`, `diff --numstat`, and three `ls-files`
variants), cached per workspace and rebuilt on every debounced
`file_tree_changed`. It has hit two walls:

1. **Scale.** `target/` in this repo is **467,438 files**. `git ls-files
   --others` enumerates all of them; we build a 467k-element `HashSet` and run
   `dir_prefixes` over all 467k paths, on every refresh. Correct, but absurd.
2. **Empty ignored dirs are invisible.** `.tau/skills` is an empty ignored
   directory, so `ls-files` never mentions it, so it is absent from
   `ignored_dirs`, so `is_ignored_dir` returns false and it is **not greyed**.
   The `ls-files`-set-difference model structurally cannot see empty ignored
   dirs — no tuning fixes it.

`gix` (the pure-Rust git library gitui is built on) fixes both at the root:

- **Prune, don't enumerate.** Check-ignore each directory while walking; an
  ignored dir (`target/`) is greyed and never descended into. The 467k files
  are never touched.
- **`is_ignored` is authoritative** for any path, empty or not. `.tau/skills`
  just works.
- **No text parsing.** The fragility already hit (C-quoted paths, the `/var`
  symlink vs `rev-parse --show-toplevel`, CRLF in `git show`) disappears —
  structured path buffers and typed trees.
- **In-process.** No subprocess spawn per snapshot.

## The contract we must NOT change

The frontend is a thin renderer over `tau-protocol`. These wire types stay
byte-identical:

- `GitStatus { branch, modified, untracked, deleted }` — `Vec<String>`,
  workspace-relative.
- `FileEntry { name, path, dir, size, ignored: bool, git: Option<FileGit> }`.
- `FileGit { state: FileGitState { Modified, Untracked, Deleted },
  added: Option<u32>, deleted: Option<u32> }`.
- `CommandOutput::GitShow { text }`.

Preserve these seams (only the `git.rs` internals change):

- `Core::git_snapshot(&workspace)` — the cache in
  `state.rs` (`git_snapshots: Mutex<HashMap<String, Option<GitSnapshot>>>`)
  plus the builder. Called by the `FileList` arm (→ `annotate`) and the
  `GitStatus` arm (→ `.status`).
- `git_show(cwd, rel) → String` — direct, from the `GitShow` arm.
- Invalidation: `watchers.rs` removes the cached snapshot on
  `file_tree_changed`, then emits `FileTreeChanged`. Unchanged.
- `list_dir` (`watchers.rs:257`) — the filesystem walk producing
  `FileEntry` with cwd-relative `path`. Unchanged.

## What changes

- `git.rs` internals: delete `run_git`, `parse_status`, `parse_numstat`,
  `untracked_sets`, `tracked_set`, `dir_prefixes`, `count_lines` (replaced by a
  gix walk). Rewrite `git_snapshot` and `git_show` on gix.
- `GitSnapshot` (an **internal** type — the wire contract is untouched): drop
  the four `HashSet` fields (`ignored`, `tracked_dirs`, `shown_dirs`,
  `ignored_dirs`). Replace with what `annotate` actually needs: a way to ask
  "is this path ignored?" + the per-file status/numstat maps.
- `annotate`: grey a node via `is_ignored(path)` (O(1) with a loaded gitignore
  stack) instead of the set lookups.
- **Delete the dead `git_diff` command** — it has no frontend caller (only
  protocol types + core + one test). Removing it drops the one hard-to-
  reproduce piece (unified-diff generation). Delete: the `Command::GitDiff`
  variant, the `CommandOutput::GitDiff` variant, the `git_diff` fn, its
  dispatch arm, its two integration tests, and the `git_diff`/`GitDiff` types
  in `protocol.ts` (both the command and the output union members).

## The gix API (verified against docs.rs, `gix 0.88.0`)

- `gix 0.88.0`, MSRV 1.88 (project is 1.98.1 — clear). Heavily feature-gated;
  enable `status`, `ignore`, `revision`, `dirwalk` (and friends) in
  `tau-core/Cargo.toml`. Also add `similar` (the line-diff crate gix itself
  uses internally) for numstat.
- `gix::open(cwd) → Repository`. A non-git path errors → map to
  `ProtocolError::Other` (matches `git_status_in_a_non_git_workspace_is_an_error`).
- `repo.status(progress) → Platform`. `platform.index` (index vs HEAD tree) and
  `platform.worktree` (worktree vs index, incl. untracked) each iterate
  `FileChange { path, status: FileStatus }`. `FileStatus` is a bitflag
  (`WD_MODIFIED`, `HD_MODIFIED`, `WD_TYPECHANGED`, `WD_DELETED`, `WD_UNTRACKED`,
  …). Map: worktree-deleted → `deleted`; untracked → `untracked`; otherwise
  modified → `modified`. **Configure the platform so untracked files are
  enumerated, not collapsed to directories** (`Platform::untracked_files` /
  `dirwalk_options`) — the current `ls-files` behaviour lists individual files.
- `repo.is_ignored(path) → bool` — authoritative ignore check (the crux of the
  fix; works for empty dirs).
- `git_show`: `repo.find_commit("HEAD")` → `.tree()` → `.find_file(rel)` →
  object → blob bytes → `String`, `\r\n`→`\n` normalized (match `file_read`).
  A path absent from HEAD errors (caller treats as "no base"/untracked).
- Toplevel offset: `repo.worktree_dir()` + the repo root give the
  cwd→toplevel relationship directly — no `canonicalize` + `strip_prefix`
  dance (the `/var` symlink case is handled by gix's path model).
- **Numstat:** for each *modified* file (a small set from status), read the
  HEAD blob + the worktree file and line-diff them with `similar`
  (`TextDiff::from_lines` → count left-only = deleted, right-only = added).
  Untracked files: `added` = line count, `deleted` = 0 (same as today's
  `count_lines`). Bounded to the changed set — never 467k.

## P0 spike findings (confirmed against `gix 0.88.0` on this repo, 2026-10-13)

The throwaway spike (`crates/tau-core/examples/gix_spike.rs`) ran the full
pipeline against this repo. **Performance win confirmed: the status walk is
46–61 ms with the 467,438 `target/` files pruned, never enumerated** (gix's
dirwalk + ignore does the pruning internally). The API shapes below are the
ground truth; where they differ from the assumptions above, this section wins.

- **Feature flags** (in `tau-core/Cargo.toml`): `gix = { version = "0.88",
  default-features = false, features = ["status", "revision", "excludes",
  "sha1"] }` + `similar = "3"` (resolves 3.2.0). `status` transitively pulls
  dirwalk/index/blob-diff; `excludes` pulls gix-ignore + gix-worktree.
- **Status walk** — the `platform.index`/`platform.worktree` + `FileStatus`
  bitflag picture above is **not** the 0.88 API. The real shape:
  - `repo.status(gix::progress::Discard)?.into_iter(std::iter::empty::<gix::bstr::BString>())?
    → Iter` yielding `Result<Item, _>` (unwrap per item). Empty pathspec = all.
  - `Item` is an **enum**: `IndexWorktree(index_worktree::Item)` (worktree vs
    index, incl. untracked) and `TreeIndex(gix_diff::index::Change)` (staged).
  - `index_worktree::Item::Modification { rela_path: &BString, status: EntryStatus, .. }`
    and `DirectoryContents { entry: gix_dir::Entry, .. }` (the untracked case).
  - `status` is `gix::status::plumbing::index_as_worktree::EntryStatus::Change(c)`
    where `c` is `plumbing::index_as_worktree::Change::{ Modification{..},
    Type{..}, Removed }` → map to modified / modified / deleted. (There is no
    `FileStatus`/`WD_*` bitflag in this form.)
  - **`plumbing` is the `gix_status` re-export** — `gix::status` does not expose
    `index_worktree`/`index_as_worktree` at the top level; they live under
    `gix::status::plumbing::`.
  - Path string: `String::from_utf8_lossy(&rela_path).into_owned()`.
  - The spike reads only `IndexWorktree` (worktree state); `TreeIndex` (staged)
    is skipped. Confirm against the 8 tests whether staged changes must fold in.
- **Ignore** — there is **no** `repo.is_ignored(path)`. The real shape:
  - `gix::ignore::Search::from_git_dir(repo.git_dir(), Some(workdir/.gitignore),
    &mut buf, gix::ignore::search::Ignore::default())?` — loads `info/exclude`
    + the **root** `.gitignore` in one call.
  - `search.pattern_matching_relative_path(gix::bstr::BStr::new(p.as_bytes()),
    Some(is_dir), gix::glob::pattern::Case::default())` → `Option<Match>`;
    ignored iff `Some(m)` and `!m.pattern.is_negative()`.
  - **A single-path check does NOT see "under an ignored dir"** (`.tau/skills`
    came back false). The fix is an **ancestor walk**: the path is ignored if it
    *or any ancestor dir* matches. Confirmed: with the walk, `target`, `.tau`,
    `.tau/skills`, `.tau/sessions`, `target/debug` → true; `crates`, `app` →
    false. This is the crux of the fix and it works.
  - **Scope limit (v0):** only the root `.gitignore` + `info/exclude` are
    consulted, so nested `.gitignore` files are not. Fine here (this repo's
    important patterns are root-level) and the status walk uses gix's full
    ignore machinery internally, so untracked enumeration stays correct. Note
    the limit in ADR-0011; upgrade path = collect nested `.gitignore` files.
- **`git_show` tree walk** — `tree.find_file(rel)` does **not** exist. Walk
  `find_entry(seg)` segment by segment: `e.kind()` →
  `gix::object::tree::EntryKind::{Tree, Blob, BlobExecutable}` (not
  `ObjectIdKind`), `e.id()` → ObjectId, then `repo.find_tree(id)` /
  `repo.find_blob(id)`. `\r\n`→`\n` normalize the blob to match `file_read`.
- **Misc**: `repo.workdir()` (not the deprecated `work_dir()`); `repo.head_name()`
  → `Option<Option<RefName>>` (flatten the inner); `gix::bstr` (BString/BStr) and
  `gix::objs` (gix_object) are public re-exports.
- **similar 3.2.0** — `TextDiff::from_lines(&old, &new)`, `iter_all_changes()` →
  `Change`; `op.tag()` → `ChangeTag::{Equal, Delete, Insert}` (there is **no**
  `Replace` — a replace is emitted as separate Delete + Insert); line count =
  `op.value().lines().count()`.

## Design decisions

1. **Prune at annotation.** `annotate` asks `is_ignored` per node; an ignored
   dir is greyed and its git-state lookup is skipped. No 467k enumeration.
2. **One `Repository` per snapshot.** Open once, reuse for status + ignore +
   (later) show. Keep the snapshot cache — status is not free — but it is now
   cheap to rebuild.
3. **Drop `git_diff`** (dead surface; see above).
4. **Record the decision as ADR-0011** ("core uses `gix` for file-tree git
   awareness"). Note: the old `git.rs` header attributed "no git library
   dependency" to ADR-0002, which is actually the single-process model — a
   misattribution. No formal ADR is being reversed; we are formalizing a new
   choice. Fix the `git.rs` header comment accordingly.

## Phases

- **P0 — Spike (throwaway).** A scratch `#[test]` or example that opens **this**
  repo with `gix` and asserts: (a) the status split is correct for a known
  modified/untracked/deleted fixture; (b) `is_ignored("target") == true` and
  `is_ignored(".tau/skills") == true`; (c) `find_commit(HEAD).tree().find_file`
  reads a known blob; (d) a line-diff numstat on a modified file; (e) **timing**
  of the status+ignore walk (confirm `target/` is pruned, not enumerated).
  Kill the spike before P1; it de-risks the API specifics.
- **P1 — Dependency + builder.** Add `gix` (+`similar`) to
  `tau-core/Cargo.toml`. Rebuild `git_snapshot` on gix, producing the same
  `GitStatus` + numstat map. Rework `GitSnapshot` to drop the four HashSets.
  Keep `git_show` on the old path for now.
- **P2 — Swap snapshot + annotate.** Point `annotate` at `is_ignored`. Get the
  8 `tests_git.rs` integration tests green, and verify live that `target/` and
  `.tau/skills` are greyed.
- **P3 — Swap `git_show` + delete `git_diff`.** `git_show` on gix. Delete the
  dead `git_diff` command/output/fn/arm/tests + `protocol.ts` types.
- **P4 — Dead-code sweep.** Remove `run_git`, `parse_status`, `parse_numstat`,
  `untracked_sets`, `tracked_set`, `dir_prefixes`, `count_lines` and the
  pure-parser unit tests in `git.rs`. Fix the header comment (ADR-0011).
- **P5 — Gate + ADR + live.** Full workspace gate (below), write ADR-0011, and
  live-verify in the running app.

## Risks

- **Status semantics vs `git status`.** The exact modified/untracked/deleted
  split and rename handling must match. The 8 integration tests are the net.
  If `gix` differs on an edge, adjust the *mapping*, not the test — unless the
  test itself is wrong (flag it, don't silently weaken it).
- **Untracked collapsing.** Must be configured off, or the untracked set comes
  back as directory names. Covered by the status test.
- **`Repository::status()` cost on huge repos.** It walks the worktree; confirm
  it prunes ignored dirs (it should, via dirwalk + ignore). The P0 spike
  measures this on this repo.
- **Dependency weight.** `gix` is sizeable (compile time + binary). Acceptable
  for a Tauri app that already links a webview; note it in ADR-0011.
- **Non-git workspaces.** `gix::open` must fail cleanly → `ProtocolError::Other`.

## Acceptance

- All 8 `tests_git.rs` integration tests pass (fixtures may need trivial
  tweaks if `gix`'s status differs on an edge — surface any such change).
- Live app: `target/` greyed **and** `.tau/skills` greyed; no 467k-file
  enumeration (the snapshot build is fast).
- No `git` subprocess spawn remains in the snapshot / show path.
- Snapshot build time on this repo measured before/after (expect a large
  improvement; record it).
- Full workspace gate green: `cargo nextest run --workspace`,
  `cargo clippy --workspace --all-targets` (`-D warnings`), and the frontend
  gate (`npm run build` / svelte-check / eslint / vitest) — the wire contract
  is unchanged, so the frontend must be untouched and still green.

## Out of scope

- Frontend changes (contract unchanged).
- The `git` CLI used by the integration tests to *create* fixture repos
  (`init_repo`) — that stays; only the core's *reads* move to `gix`.
