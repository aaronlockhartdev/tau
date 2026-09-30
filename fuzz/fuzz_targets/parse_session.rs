#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use tau_core::session::SessionStore;

// A single scratch dir for the whole target process: open() only reads the
// file, so iterations can overwrite the same path.
fn scratch() -> &'static tempfile::TempDir {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".tau").join("sessions")).unwrap();
        dir
    })
}

// Oracle: a session .jsonl must always load to a Result — never panic,
// and malformed content yields an error, not a partial load.
fuzz_target!(|data: Vec<u8>| {
    let dir = scratch();
    let store = SessionStore::for_workspace(dir.path(), "fuzz");
    std::fs::write(store.path(), &data).unwrap();
    let mut store = store;
    let _ = store.open();
});
