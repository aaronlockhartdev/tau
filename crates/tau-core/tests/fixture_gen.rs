//! The shared 10k-entry session fixture (roadmap G, handoff 1): one real
//! `SessionStore` write, deterministically placed at
//! `target/test-fixture/session.jsonl` and hashed, for the GUI's real-app
//! E2E to consume. The bytes must not depend on when the test runs — the
//! fixed clock is what makes a re-run byte-identical (the Rust tests, the
//! snapshot test, and the E2E all read this one artifact).

use sha2::{Digest, Sha256};
use std::path::Path;
use tau_core::session::SessionStore;

/// The fixture's fixed clock (epoch ms).
const FIXED_TIME: u64 = 1_750_000_000_000;

#[test]
fn the_shared_fixture_is_written_and_hashed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(tmp.path(), "session").with_fixed_time(FIXED_TIME);
    store.create().unwrap();
    let mut prev: Option<String> = None;
    for i in 0..10_000u64 {
        let text = format!("entry {i:05} ") + &"p".repeat(150);
        let e = store
            .append(
                "message",
                serde_json::json!({"role": "user", "text": text}),
                prev.as_deref(),
            )
            .unwrap();
        prev = Some(e.id);
    }

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
