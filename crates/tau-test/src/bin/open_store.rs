//! The setup child: it opens the store at a path and prints a line when the store is
//! open, so the lane watches a second process opening a store whose setup is locked by
//! another (R-session-33). Its line carries the refusals the setup waited out; failure
//! exits 1 with the reason on stderr.

use std::io::Write;
use tau_core::storage::Store;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let mut out = std::io::stdout();
    // Printed before the open, so a parent watching the pipe knows this instance has
    // reached the store setup, and is not merely started.
    let _ = writeln!(out, "opening");
    let _ = out.flush();
    match Store::open(&path) {
        Ok(store) => {
            // The refusals ride on the line, so a parent can tell a met lock from a race
            // that never met one: refusals=0 means this instance never waited.
            let _ = writeln!(out, "opened refusals={}", store.setup_refusals());
            let _ = out.flush();
        }
        Err(e) => {
            eprintln!("open_store: {e}");
            std::process::exit(1);
        }
    }
}
