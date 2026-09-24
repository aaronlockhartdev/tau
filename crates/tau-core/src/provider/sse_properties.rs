use super::*;
use proptest::prelude::*;

/// One valid stream over every frame dialect the parser special-cases:
/// comments, JSON deltas, multi-byte UTF-8, a multi-line data event, and
/// the `[DONE]` terminator.
fn body(frames: &[usize]) -> String {
    let mut out = String::new();
    for f in frames {
        out.push_str(match f {
            0 => ": keep-alive comment\n\n",
            1 => "data: {\"type\":\"response.created\"}\n\n",
            2 => "data: {\"type\":\"response.output_text.delta\",\"delta\":\"word \"}\n\n",
            3 => "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"thought éé✓\"}\n\n",
            4 => "data: part1\ndata: part2\n\n",
            _ => "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        });
    }
    out.push_str("data: [DONE]\n\n");
    out
}

proptest! {
    /// Parse + coalesce is invariant to how the stream is chunked:
    /// every byte-level partition of one valid body yields the same
    /// decoded event sequence (generalizes the UTF-8-split case).
    #[test]
    fn chunk_boundaries_do_not_change_the_decoded_events(
        (n, chunk) in (1usize..16, 1usize..40),
    ) {
        let frames: Vec<usize> = (0..n).map(|i| (i * 7) % 6).collect();
        let full = body(&frames);
        let bytes = full.as_bytes();

        let mut reference = SseParser::new();
        let expected = reference.feed(bytes).unwrap();
        let reference_terminated = reference.terminated;

        let mut chunked = SseParser::new();
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < bytes.len() {
            let end = (pos + chunk).min(bytes.len());
            out.extend(chunked.feed(&bytes[pos..end]).unwrap());
            pos = end;
        }
        prop_assert_eq!(out, expected);
        prop_assert_eq!(chunked.terminated, reference_terminated);
    }
}
