## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues — the wayfinder maps and their child tickets among them — driven via the `gh` CLI. See `docs/agents/issue-tracker.md` (conventions + wayfinding operations).

### Triage labels

Five triage roles: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` (the glossary), `docs/spec/v0.md` (the approved v0 contract), and `docs/adr/` at the repo root. The v0 clean-up works from `docs/v0-cleanup-roadmap.md`; maps and tickets live in GitHub Issues. See `docs/agents/domain.md`.

### tauri-pilot

To inspect, drive, or screenshot a running dev build, load the `tauri-pilot` skill. The app embeds its plugin in debug builds only; the CLI auto-detects the socket.

## Coding conventions

Rules, not suggestions. Enforced in CI where mechanical, in review where not.

**Comments — anti-slop discipline.**
- A comment is justified only by a *why* that is not obvious from the code: a constraint, a workaround, a non-obvious invariant, a decision citation (spec §/ADR/ticket).
- Never narrate what the code does ("loads the config"), never restate the name in prose, never use banner or section-divider comments, never write "Adds/Updates X" change narration.
- No speculative TODOs — open the ticket instead. A TODO that names no ticket and no reason is noise.

**Naming over commenting.** If a name needs a comment to be understood, fix the name first.

**Formatting.** Rust: `rustfmt` defaults + `clippy -D warnings`, both enforced in CI. Svelte/TypeScript: same spirit — consistent 2-space indent, single quotes, semicolons on; no extra formatter tooling in v0.

**File editing.** Repo files are edited with the file tools (read → edit with anchors, or write) — never with shell scripts (`sed`, `python`, `awk`). The file tools are atomic, leave a reviewable diff, and surface conflicts a silent in-place rewrite hides.
**No speculative code.** No dead abstractions, no "for the future" scaffolding, no flags for nonexistent features, no second implementation kept "just in case". Build the thing the ticket asks for; the next ticket extends it.

**File size.** Every code file stays under a soft 500-LOC limit and a hard 1000-LOC limit. The soft limit is a review smell that invites a split; the hard limit is a blocker, enforced in CI by the `file-size` job: any code file over 1000 LOC fails the build. No grandfathering — the ten files that started the v0 clean-up over the hard limit (`app/src-tauri/src/core.rs` at 6,297 being the largest) are all split, and the gate has been live since the v0 clean-up's Wave 4.

**Doc comments** appear only on public API items whose contract is not self-evident from the signature — one line where possible, with a spec/ADR citation when a rule comes from one.

## Verifying UI work

Confirm a visual bug by taking a screenshot of the running app and reading it — a green DOM assertion does not prove the UI renders correctly.
