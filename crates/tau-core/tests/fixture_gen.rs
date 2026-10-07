//! The shared 10k-entry session fixture (roadmap G, handoff 1): one real
//! `SessionStore` write, deterministically placed at
//! `target/test-fixture/session.jsonl` and hashed, for the GUI's real-app
//! E2E to consume. The bytes must not depend on when the test runs — the
//! fixed clock is what makes a re-run byte-identical (the Rust tests, the
//! snapshot test, and the E2E all read this one artifact).
#![allow(clippy::unwrap_used, clippy::panic)]
//!
//! The content is a realistic long-running session: 50 dev goals, each
//! worked through 200 entries (a user goal, a plan, a 197-call tool loop,
//! a summary) on the real entry kinds and payload shapes, so the GUI
//! renders it like an ordinary transcript. Each goal also ends in a
//! compaction record (`om` entry, the shape `OmState::save` writes):
//! a long session compacts many times, and the final goal's record
//! exceeds the blob threshold, at the session's tail, so boot hydration
//! exercises the zstd sidecar (#49).
//!
#![allow(
    clippy::cast_possible_truncation,
    reason = "deterministic fixture: loop indices are bounded by the fixed 50x200 shape"
)]

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::Path;
use tau_core::session::SessionStore;

/// The fixture's fixed clock (epoch ms).
const FIXED_TIME: u64 = 1_750_000_000_000;

const GOALS: &[&str] = &[
    "Fix the flake in the SSE parser test",
    "Refactor the session store paging to the cursor API",
    "Add retry with backoff to the provider client",
    "Split the harness dispatch into per-command modules",
    "Make the session tree in the left pane lazy-load",
    "Fix the file watcher memory leak",
    "Add property tests for the hashline format",
    "Virtualize the transcript entry list",
    "Fix the doubled track height on boot",
    "Add a one-line summary for archived sessions",
    "Make long tool outputs collapsible in the card",
    "Fix the race in the atomic rewrite",
    "Track usage per turn in the status bar",
    "Make the config merge honor the project layer",
    "Add a smoke test for the pilot socket",
    "Cut the store bundle size in half",
    "Fix the scroll jump after a session refresh",
    "Sort the task list by most-recent use",
    "Add a shortcut to switch sessions",
    "Fix the double render on session open",
    "Debounce the file tree refetch",
    "Fold om reflections away by default",
    "Stabilize the blob threshold constant",
    "Use a deterministic clock in the session tests",
    "Cache rendered entry heights in the card",
];

const FILES: &[&str] = &[
    "src/harness/turn.rs",
    "src/harness/pump.rs",
    "src/agent.rs",
    "src/session.rs",
    "src/provider.rs",
    "src/subagent.rs",
    "src/om.rs",
    "src/task.rs",
    "svelte/lib/entries.ts",
    "svelte/components/EntryCard.svelte",
    "svelte/lib/store.svelte",
    "tests/retry.rs",
];

const COMMANDS: &[&str] = &[
    "cargo test -p tau-core -- sse",
    "cargo test -p tau-core -- fixture",
    "cargo clippy -p tau-core --all-targets",
    "cargo fmt --check",
    "npm run build",
    "npm run test",
    "git diff --stat",
    "git status --short",
];

const OUTPUTS: &[&str] = &[
    "test result: ok. 253 passed; 0 failed; 0 ignored",
    "test result: ok. 8 passed; 0 failed; 0 ignored",
    "Finished `dev` profile in 3.42s",
    "0 errors; 0 warnings",
    "exit 0",
    "12 files changed, 214 insertions(+), 87 deletions(-)",
    "src/agent.rs:33:pub const KIND_TOOL: &str = \"tool\";",
    "M  src/harness/turn.rs",
];

// Observation lines for the compaction records: static (determinism is
// contractual, ADR-0009). The GUI's 'om' decoder shows the newest one —
// past the last message boundary (entries.ts).
const OM_OBS: &[&str] = &[
    "the SSE parser flake reproduces only under a 50 ms timer avg",
    "the session store paging test needs the cursor API before it can assert",
    "the provider client retry is bounded at 3 attempts in the current code",
    "the harness dispatch split leaves turn.rs at 385 lines",
    "the left-pane tree loads every row eagerly on workspace open",
    "the file watcher holds a handle per watched dir; the leak is the unwatch path",
    "the hashline format property tests cover round-trip but not the CRC edge",
    "the transcript virtualizer estimates 40 px per entry before measurement",
    "the doubled track height appears only when the boot pin scrolls twice",
    "the archived session summary line is the first assistant text, truncated",
    "the card collapse state is per-entry and not persisted across re-open",
    "the atomic rewrite settles the tail before the next append is visible",
    "the status bar usage group reads the session meta, not the stream",
    "the config merge keeps the system layer's provider order in a BTreeMap",
    "the pilot socket name must fit the 104-byte SUN_PATH on macOS",
];
const OM_SUGGESTED: &[&str] = &[
    "re-run the failing suite with the timer probe enabled",
    "split the dispatch module before adding the next command",
    "add the cursor API to the store and port the paging test",
    "watch the next turn's first frame for the height inflation",
];

// One compaction record per goal: the payload `OmState::save` writes
// (om_integration.rs) — active_observations with a message boundary so
// the GUI's 'om' decoder shows the newest observation. The final goal
// carries a long observation log past the blob threshold, so its payload
// goes to a zstd sidecar at the session's tail — the boot pin's window
// resolves it on hydration (#49).
fn om_payload(goal: u64) -> serde_json::Value {
    const BOUNDARY: &str = "--- message boundary (2025-06-15T15:33:20.000Z) ---";
    let rounds = if goal == 49 { 2_000 } else { 2 };
    let mut older = String::new();
    for i in 0..rounds {
        writeln!(
            &mut older,
            "- {}",
            OM_OBS[usize::try_from(i).expect("loop bound") % OM_OBS.len()]
        )
        .expect("writing to a String cannot fail");
    }
    let newest = OM_OBS[usize::try_from(goal).expect("loop index 0..50") % OM_OBS.len()];
    serde_json::json!({
        "active_observations": format!(
            "<observation-group>\n{older}\n\n{BOUNDARY}\n\n<observation-group>\n- {newest}\n</observation-group>"
        ),
        "om_suggested_response": OM_SUGGESTED[usize::try_from(goal).expect("loop index 0..50") % OM_SUGGESTED.len()],
        "om_model": "gpt-5.2"
    })
}

#[test]
#[allow(clippy::too_many_lines)] // one fixture-generation pass; splitting is refactoring
fn the_shared_fixture_is_written_and_hashed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(tmp.path(), "session").with_fixed_time(FIXED_TIME);
    store.create().unwrap();
    let mut prev: Option<String> = None;
    let mut n = 0u64;
    for g in 0..50u64 {
        let goal = GOALS[usize::try_from(g).expect("loop index 0..50") % GOALS.len()];
        let file = FILES[usize::try_from(g).expect("loop index 0..50") % FILES.len()];
        let e = store
            .append(
                "user",
                serde_json::json!({ "text": goal, "lane": "follow-up" }),
                prev.as_deref(),
            )
            .unwrap();
        prev = Some(e.id);
        n += 1;
        let e = store
            .append(
                "assistant",
                serde_json::json!({
                    "text": format!("Plan: start with {file}, then work the loop to completion."),
                    "reasoning": "",
                    "interrupted": false,
                    "calls": []
                }),
                prev.as_deref(),
            )
            .unwrap();
        prev = Some(e.id);
        n += 1;
        for k in 0..197u64 {
            let tool = match k % 5 {
                0 => serde_json::json!({
                    "call_id": format!("call-{n}"),
                    "name": "read",
                    "args": { "path": file },
                    "output": "200 lines read"
                }),
                1 => serde_json::json!({
                    "call_id": format!("call-{n}"),
                    "name": "grep",
                    "args": { "pattern": "KIND_TOOL", "path": "src/" },
                    "output": OUTPUTS[usize::try_from(k).expect("loop index 0..197") % OUTPUTS.len()]
                }),
                2 => serde_json::json!({
                    "call_id": format!("call-{n}"),
                    "name": "edit",
                    "args": { "file": file, "anchor": format!("L{}", 100 + k) },
                    "output": "applied"
                }),
                3 => serde_json::json!({
                    "call_id": format!("call-{n}"),
                    "name": "bash",
                    "args": { "command": COMMANDS[usize::try_from(k).expect("loop index 0..197") % COMMANDS.len()] },
                    "output": OUTPUTS[usize::try_from(k + 3).expect("loop index 0..197 + 3") % OUTPUTS.len()]
                }),
                _ => serde_json::json!({
                    "call_id": format!("call-{n}"),
                    "name": "write",
                    "args": { "path": "tests/retry.rs" },
                    "output": "written"
                }),
            };
            let e = store.append("tool", tool, prev.as_deref()).unwrap();
            prev = Some(e.id);
            n += 1;
        }
        // Compaction after the long tool loop, before the summary: the
        // record is the newest `om` entry, so re-opens read it back through
        // the blob sidecar when it is big enough (#49).
        let e = store.append("om", om_payload(g), prev.as_deref()).unwrap();
        prev = Some(e.id);
        n += 1;
        let e = store
            .append(
                "assistant",
                serde_json::json!({
                    "text": format!("Done: {goal} — verified on a re-run."),
                    "reasoning": "",
                    "interrupted": false,
                    "calls": []
                }),
                prev.as_deref(),
            )
            .unwrap();
        prev = Some(e.id);
        n += 1;
    }
    assert_eq!(n, 10_050);
    // The final goal's record is the sidecar case: it must clear the blob
    // threshold, or the fixture silently stops exercising it.
    assert!(
        u64::try_from(om_payload(49).to_string().len())
            .expect("payload length, well under u64::MAX")
            > tau_core::session::DEFAULT_BLOB_THRESHOLD,
        "the final goal's compaction record must exceed the blob threshold"
    );

    let root = std::env::var("CARGO_TARGET_DIR").ok().map_or_else(
        || {
            // The crate's manifest dir is `{workspace}/crates/tau-core`.
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target")
        },
        std::path::PathBuf::from,
    );
    let dir = root.join("test-fixture");
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = std::fs::read(store.path()).unwrap();
    std::fs::write(dir.join("session.jsonl"), &bytes).unwrap();
    // The oversized compaction record's zstd sidecar (ADR-0005) is a
    // separate file under the store root: ship it with the fixture, or
    // the E2E hydration cannot resolve the blob (#49).
    let blobs = tmp.path().join(".tau").join("blobs");
    if blobs.is_dir() {
        let out = dir.join("blobs");
        // Regenerate the sidecar set from scratch — a stale one from an
        // older run must not ride into the shipped fixture.
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        for entry in std::fs::read_dir(&blobs).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), out.join(entry.file_name())).unwrap();
        }
    }
    let digest = Sha256::digest(&bytes);
    println!(
        "shared fixture: {} (sha256 {digest:x})",
        dir.join("session.jsonl").display()
    );
}
