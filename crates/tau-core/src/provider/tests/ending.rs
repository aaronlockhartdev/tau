use super::server::*;
use super::*;

#[tokio::test]
async fn mid_stream_error_never_terminates_the_stream() {
    let body = ok_body(&[
        r#"{"type":"response.created"}"#,
        r#"{"type":"response.output_text.delta","delta":"part1 "}"#,
        r#"{"type":"error","error":{"message":"boom"}}"#,
        r#"{"type":"response.output_text.delta","delta":"part2"}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}"#,
        "[DONE]",
    ]);
    let (base, _) = mock_server(vec![body]).await;
    let request = ResponseRequest::new(
        "m",
        None,
        vec![InputMessage {
            role: "user".into(),
            content: "hi".into(),
        }]
        .into_iter()
        .map(InputEntry::Message)
        .collect(),
    );
    let result = stream_response(
        &reqwest::Client::new(),
        &provider_for(&base),
        &fast_requests(),
        &request,
    )
    .await
    .unwrap();
    assert_eq!(result.text, "part1 part2");
    assert_eq!(result.mid_stream_errors, vec!["boom".to_string()]);
    assert_eq!(result.usage.unwrap().total_tokens(), 3);
    // A completion frame after the error names the ending: the response made
    // it to an end, whatever the mid-stream error was (#307).
    assert_eq!(result.outcome, StreamOutcome::Completed);
}

#[tokio::test]
async fn an_error_frame_with_nothing_after_it_names_the_ending() {
    // The provider apologises mid-stream and the body then just stops: the
    // break is the provider's error, not a silent EOF.
    let body = ok_body(&[
        r#"{"type":"response.created"}"#,
        r#"{"type":"response.output_text.delta","delta":"part1"}"#,
        r#"{"type":"error","error":{"message":"upstream died"}}"#,
    ]);
    let (base, _) = mock_server(vec![body]).await;
    let request = ResponseRequest::new(
        "m",
        None,
        vec![InputMessage {
            role: "user".into(),
            content: "hi".into(),
        }]
        .into_iter()
        .map(InputEntry::Message)
        .collect(),
    );
    let result = stream_response(
        &reqwest::Client::new(),
        &provider_for(&base),
        &fast_requests(),
        &request,
    )
    .await
    .unwrap();
    assert_eq!(result.text, "part1");
    assert_eq!(result.outcome, StreamOutcome::MidStreamError);
}

/// One canned body through the live read loop: what a provider turn says about
/// itself when the frames are served whole.
async fn one_turn(frames: &[&str]) -> TurnResult {
    let (base, _count) = mock_server(vec![ok_body(frames)]).await;
    let request = ResponseRequest::new(
        "m",
        None,
        vec![InputMessage {
            role: "user".into(),
            content: "hi".into(),
        }]
        .into_iter()
        .map(InputEntry::Message)
        .collect(),
    );
    stream_response(
        &reqwest::Client::new(),
        &provider_for(&base),
        &fast_requests(),
        &request,
    )
    .await
    .expect("the body is served whole")
}

#[tokio::test]
async fn a_capped_response_keeps_its_cap_past_a_clean_end() {
    let result = one_turn(&[
        r#"{"type":"response.created"}"#,
        r#"{"type":"response.output_text.delta","delta":"trunc"}"#,
        r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":2,"output_tokens":4}}}"#,
        "[DONE]",
    ])
    .await;
    assert_eq!(result.outcome, StreamOutcome::Length);
    assert_eq!(
        result.usage.unwrap().output_tokens,
        4,
        "a capped turn still spent tokens"
    );
}

#[tokio::test]
async fn an_incomplete_response_naming_no_cap_is_still_no_completion() {
    // A content filter, a policy stop, or a server that omits the reason
    // entirely: the provider stopped the response, and a following [DONE] does
    // not make it complete (#307).
    let result = one_turn(&[
        r#"{"type":"response.created"}"#,
        r#"{"type":"response.output_text.delta","delta":"refus"}"#,
        r#"{"type":"response.incomplete","response":{"usage":{"input_tokens":2,"output_tokens":1}}}"#,
        "[DONE]",
    ])
    .await;
    assert_eq!(result.outcome, StreamOutcome::EofWithoutDone);
}

#[tokio::test]
async fn a_later_error_frame_does_not_unname_a_completed_response() {
    let result = one_turn(&[
        r#"{"type":"response.created"}"#,
        r#"{"type":"response.output_text.delta","delta":"whole"}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        r#"{"type":"error","error":{"message":"trailing frame"}}"#,
        "[DONE]",
    ])
    .await;
    assert_eq!(result.outcome, StreamOutcome::Completed);
    assert_eq!(result.mid_stream_errors, vec!["trailing frame".to_string()]);
}

#[tokio::test]
async fn an_incomplete_response_that_never_sends_done_names_itself() {
    // The other half of the same shape: an incomplete frame with no reason,
    // and no [DONE] after it (#307).
    let result = one_turn(&[
        r#"{"type":"response.output_text.delta","delta":"half"}"#,
        r#"{"type":"response.incomplete","response":{}}"#,
    ])
    .await;
    assert_eq!(result.outcome, StreamOutcome::EofWithoutDone);
}

#[tokio::test]
async fn the_first_terminal_frame_names_the_ending() {
    // Two terminal frames is a server contradicting itself; the first is its
    // account of the ending and stands (#307).
    let result = one_turn(&[
        r#"{"type":"response.output_text.delta","delta":"half"}"#,
        r#"{"type":"response.incomplete","response":{}}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
        "[DONE]",
    ])
    .await;
    // The incomplete frame named no cap, so no completion — and the later
    // completed frame cannot unname it.
    assert_eq!(result.outcome, StreamOutcome::EofWithoutDone);
}

#[tokio::test]
async fn a_done_terminated_stream_without_completed_frame_is_complete() {
    // The exact dogfood shape: a live provider ends with [DONE] but omits
    // the response.completed frame (common on tool-call responses). The
    // stream is complete — not an interrupted turn.
    let result = one_turn(&[
        r#"{"type":"response.output_text.delta","delta":"hello"}"#,
        "[DONE]",
    ])
    .await;
    assert_eq!(result.text, "hello");
    assert_eq!(
        result.outcome,
        StreamOutcome::Completed,
        "[DONE] ends the stream cleanly even without response.completed"
    );
}

#[tokio::test]
async fn a_stop_after_an_error_frame_is_the_users_cut() {
    // The provider said the stream went wrong, then the user stopped: the stop
    // is the ending the record names, as a user cut outranks a provider cut
    // there too (#307).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let body = concat!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n",
            "data: {\"type\":\"error\",\"error\":{\"message\":\"upstream blew up\"}}\n\n"
        );
        let _ = sock.write_all(body.as_bytes()).await;
        let _ = sock.flush().await;
        // Silent after the error frame: only the stop ends the turn.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let result = stream_turn(
        &reqwest::Client::new(),
        &provider_for(&format!("http://{addr}")),
        &Requests {
            timeout_secs: 30,
            retries: 2,
            idle_timeout_secs: 30,
            tool_batch_on_force: crate::config::ToolBatchPolicy::default(),
        },
        &ResponseRequest::new("m", None, vec![]),
        &mut StopAt {
            at: std::time::Instant::now() + Duration::from_millis(200),
        },
    )
    .await
    .expect("the request was accepted");
    assert_eq!(
        result.outcome,
        StreamOutcome::Killed,
        "the user's stop is the ending"
    );
    assert!(
        !result.mid_stream_errors.is_empty(),
        "the error frame was still seen"
    );
}
