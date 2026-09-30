#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_core::provider::SseParser;

// Oracle: feed is chunk-boundary independent — the contract pinned by
// provider::sse_properties::chunk_boundaries_do_not_change_the_decoded_events.
// A one-shot feed and a 7-byte-chunked feed of the same bytes must decode
// identically, and neither may panic.
fuzz_target!(|data: Vec<u8>| {
    let mut whole = SseParser::new();
    let whole_frames = whole.feed(&data).unwrap_or_default();

    let mut chunked = SseParser::new();
    let mut chunked_frames = Vec::new();
    for chunk in data.chunks(7) {
        chunked_frames.extend(chunked.feed(chunk).unwrap_or_default());
    }

    assert_eq!(whole_frames, chunked_frames);
    assert_eq!(whole.terminated, chunked.terminated);
});
