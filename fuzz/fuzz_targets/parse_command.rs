#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_protocol::Command;

// Oracle: a command wire payload must always decode to a Result, never panic.
fuzz_target!(|data: Vec<u8>| {
    let _ = serde_json::from_slice::<Command>(&data);
});
