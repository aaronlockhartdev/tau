use super::*;

mod canned;
mod models;
mod server;
mod wire;

/// Live tests use a no-pool client: a pooled keep-alive connection keeps
/// the tokio runtime alive after the test and hangs teardown (ticket #23).
pub(crate) fn test_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .expect("test client")
}

struct Keep;
impl TurnSink for Keep {
    fn event(&mut self, _: TurnEvent) -> bool {
        true
    }
}

#[test]
fn endpoint_url_trims_trailing_slash() {
    assert_eq!(endpoint_url("http://x/v1/", "models"), "http://x/v1/models");
    assert_eq!(endpoint_url("http://x/v1", "models"), "http://x/v1/models");
}

fn sse(event: &str) -> String {
    format!("data: {event}\n\n")
}

#[test]
fn sse_parses_data_events_and_joins_multiline_data() {
    let mut p = SseParser::new();
    let events = p
        .feed(sse(r#"{"type":"response.created"}"#).as_bytes())
        .unwrap();
    assert_eq!(events.len(), 1);
    let events = p
        .feed(b"data: part1\ndata: part2\n\ndata: [DONE]\n\n")
        .unwrap();
    assert_eq!(
        events,
        vec!["part1\npart2".to_string(), "[DONE]".to_string()]
    );
    assert!(p.terminated);
}

#[test]
fn sse_skips_comment_lines() {
    let mut p = SseParser::new();
    let events = p.feed(b": OPENROUTER PROCESSING\n\n").unwrap();
    assert!(events.is_empty());
    let events = p
        .feed(sse(r#"{"type":"response.output_text.delta","delta":"x"}"#).as_bytes())
        .unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn sse_buffers_utf8_split_across_chunks() {
    let mut p = SseParser::new();
    let full = sse(r#"{"type":"response.output_text.delta","delta":"éé✓"}"#);
    // Split the payload mid-code-point (é = 2 bytes).
    let cut = full.len() - 12;
    let events = p.feed(&full.as_bytes()[..cut]).unwrap();
    assert!(events.is_empty());
    let events = p.feed(&full.as_bytes()[cut..]).unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].contains("éé✓"));
}

#[test]
fn sse_done_sets_terminated() {
    let mut p = SseParser::new();
    assert!(!p.terminated);
    p.feed(sse("[DONE]").as_bytes()).unwrap();
    assert!(p.terminated);
}

#[test]
fn a_completed_frame_carries_usage_in_either_spelling() {
    let responses: Frame = serde_json::from_str(
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":11,"output_tokens":5,"total_tokens":16}}}"#,
    )
    .expect("the Responses shape decodes");
    let chat: Frame = serde_json::from_str(
        r#"{"type":"response.completed","response":{"usage":{"prompt_tokens":11,"completion_tokens":5,"total_tokens":16}}}"#,
    )
    .expect("the chat-completions dialect decodes");
    for frame in [responses, chat] {
        let Frame::Completed { response } = frame else {
            unreachable!("both bodies are response.completed")
        };
        let usage = response
            .and_then(|r| r.usage)
            .map(super::usage::UsageWire::into_usage)
            .expect("a usage object");
        assert_eq!((usage.input_tokens, usage.output_tokens), (11, 5));
        assert_eq!(usage.total_tokens(), 16, "the sum, not the sent total");
        assert_eq!(usage.cached_input_tokens, None, "nothing was said");
    }
}

#[test]
fn reasoning_dialects_normalize_into_one_field() {
    for field in ["reasoning", "reasoning_content", "thinking"] {
        let payload = format!(
            r#"{{"type":"response.output_text.delta","delta":"answer","{field}":"thought"}}"#
        );
        let mut result = TurnResult::default();
        let mut created_at: Option<i64> = None;
        let mut ending = Ending::default();
        apply_frame(
            &mut Keep,
            &mut result,
            &mut created_at,
            &mut ending,
            &payload,
        );
        assert_eq!(result.text, "answer", "dialect {field}");
        assert_eq!(result.reasoning, "thought", "dialect {field}");
    }
    let payload = r#"{"type":"response.output_text.delta","delta":"answer",
        "reasoning_details": [{"text": "thought-a"}, {"text": "thought-b"}]}"#;
    let mut result = TurnResult::default();
    let mut created_at: Option<i64> = None;
    let mut ending = Ending::default();
    apply_frame(
        &mut Keep,
        &mut result,
        &mut created_at,
        &mut ending,
        payload,
    );
    assert_eq!(result.reasoning, "thought-athought-b");
}
