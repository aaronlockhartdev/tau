#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_core::hashline::{self, Edit};

// Oracle: the anchor allocation and edit resolution are total over their
// defined inputs — a random content string and a random (possibly malformed
// or stale) anchor pair must never panic. Errors are the contract; a panic
// is a bug (an out-of-bounds probe, a bad UTF-8 split, a truncation edge).
fuzz_target!(|data: Vec<u8>| {
    let content = String::from_utf8_lossy(&data).into_owned();

    // Allocation: line_hashes is total over any content.
    let _ = hashline::line_hashes(&content);
    let _ = hashline::render(&content);

    // The 500-byte hash source is a char-boundary truncate: total over
    // any line, including multibyte sequences straddling the cut.
    for line in content.lines() {
        let _ = hashline::hash_source(line);
        let _ = hashline::canon(line);
    }

    // Resolution: a random anchor (letters only, length-bounded) against the
    // file must resolve to Ok(slot) or a typed Err, never panic. A 4-letter
    // draw from the alphabet is the shape the model produces; anything else
    // is BadAnchor.
    let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let anchor: String = data
        .iter()
        .take(8)
        .map(|&b| alphabet[(b % alphabet.len() as u8) as usize] as char)
        .collect();
    let edit = Edit {
        from: anchor.clone(),
        to: anchor,
        content: content.clone(),
    };
    let _ = hashline::apply_edit(&content, &edit);
});
