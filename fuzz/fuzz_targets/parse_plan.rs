#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_core::plan;

// Oracle: a plan file must always parse (the total function) and render —
// never panic, on any byte sequence.
fuzz_target!(|data: Vec<u8>| {
    let text = String::from_utf8_lossy(&data);
    let plan = plan::parse(&text);
    let _ = plan::render(&plan);
});
