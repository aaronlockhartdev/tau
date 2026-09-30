#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_protocol::Event;

// Oracle: an event wire payload must always decode to a Result, never panic.
fuzz_target!(|data: Vec<u8>| {
    let _ = serde_json::from_slice::<Event>(&data);
});
