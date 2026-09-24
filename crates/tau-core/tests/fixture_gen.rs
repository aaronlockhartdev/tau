//! The shared 10k-entry session fixture (roadmap G, handoff 1): one real
//! `SessionStore` write, deterministically placed at
//! `target/test-fixture/session.jsonl` and hashed, for the GUI's real-app
//! E2E to consume. The bytes must not depend on when the test runs — the
//! fixed clock is what makes a re-run byte-identical (the Rust tests, the
//! snapshot test, and the E2E all read this one artifact).
//!
//! The content is a realistic long-running session: 50 dev goals, each
//! worked through 200 entries (a user goal, a plan, a 197-call tool loop,
//! a summary) on the real entry kinds and payload shapes, so the GUI
//! renders it like an ordinary transcript.

use sha2::{Digest, Sha256};
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

#[test]
fn the_shared_fixture_is_written_and_hashed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(tmp.path(), "session").with_fixed_time(FIXED_TIME);
    store.create().unwrap();
    let mut prev: Option<String> = None;
    let mut n = 0u64;
    for g in 0..50u64 {
        let goal = GOALS[g as usize % GOALS.len()];
        let file = FILES[g as usize % FILES.len()];
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
                    "output": OUTPUTS[k as usize % OUTPUTS.len()]
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
                    "args": { "command": COMMANDS[k as usize % COMMANDS.len()] },
                    "output": OUTPUTS[(k + 3) as usize % OUTPUTS.len()]
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
    assert_eq!(n, 10_000);

    let root = std::env::var("CARGO_TARGET_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            // The crate's manifest dir is `{workspace}/crates/tau-core`.
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target")
        });
    let dir = root.join("test-fixture");
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = std::fs::read(store.path()).unwrap();
    std::fs::write(dir.join("session.jsonl"), &bytes).unwrap();

    let digest = Sha256::digest(&bytes);
    println!(
        "shared fixture: {} (sha256 {digest:x})",
        dir.join("session.jsonl").display()
    );
}
