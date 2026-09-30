#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_protocol::Command;

// Oracle: parsing is normalizing — re-serializing a parsed command and
// parsing that form again must yield a byte-identical wire form.
fuzz_target!(|data: Vec<u8>| {
    let Ok(cmd) = serde_json::from_slice::<Command>(&data) else {
        return;
    };
    let first = serde_json::to_string(&cmd).unwrap();
    let reparsed: Command = serde_json::from_str(&first).unwrap();
    let second = serde_json::to_string(&reparsed).unwrap();
    assert_eq!(first, second);
});
