use super::server::*;
use super::*;

#[test]
fn decode_stream_extracts_function_calls_and_reasoning_dialect() {
    let body = ok_body(&[
        r#"{"type":"response.output_item.added","item":{"id":"i1","type":"function_call","name":"bash","call_id":"c1","arguments":""}}"#,
        r#"{"type":"response.function_call_arguments.delta","delta":"{\"command\": \"ls\"}","item_id":"i1"}"#,
        r#"{"type":"response.output_item.done","item":{"id":"i1","type":"function_call","name":"bash","call_id":"c1","arguments":"{\"command\": \"ls\"}"}}"#,
        r#"{"type":"response.reasoning_text.delta","delta":"thinking"}"#,
        r#"{"type":"response.output_text.delta","delta":"done"}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}}"#,
        "[DONE]",
    ]);
    let (events, calls) = decode_stream(&body).unwrap();
    assert_eq!(
        events,
        vec![
            TurnEvent::Reasoning("thinking".into()),
            TurnEvent::Text("done".into()),
            TurnEvent::Completed(Usage {
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
                ..Default::default()
            })
        ]
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.name, "bash");
    assert_eq!(calls[0].1.call_id, "c1");
    assert_eq!(calls[0].1.arguments, r#"{"command": "ls"}"#);
}

#[test]
fn canned_cut_leaves_the_partial_incomplete() {
    let body = ok_body(&[
        r#"{"type":"response.output_text.delta","delta":"par"}"#,
        r#"{"type":"response.output_text.delta","delta":"tial"}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
        "[DONE]",
    ]);
    let provider = canned_cut(&body, 1);
    let request = ResponseRequest::new("m", None, vec![]);
    let result = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            struct Keep;
            impl TurnSink for Keep {
                fn event(&mut self, _: TurnEvent) -> bool {
                    true
                }
            }
            provider.call(&request, &mut Keep).await.unwrap()
        })
    };
    assert_eq!(result.text, "par");
    assert!(!result.completed, "a cut stream is not completed");
    assert!(
        result.usage.is_none(),
        "usage commits only from response.completed"
    );

    let provider = canned(&body);
    let result = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            struct Keep;
            impl TurnSink for Keep {
                fn event(&mut self, _: TurnEvent) -> bool {
                    true
                }
            }
            provider.call(&request, &mut Keep).await.unwrap()
        })
    };
    assert_eq!(result.text, "partial");
    assert!(result.completed);
}

#[tokio::test]
async fn sink_kill_stops_the_stream_with_a_partial() {
    let body = ok_body(&[
        r#"{"type":"response.output_text.delta","delta":"abc"}"#,
        r#"{"type":"response.output_text.delta","delta":"def"}"#,
        r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
        "[DONE]",
    ]);
    let (base, _) = mock_server(vec![body]).await;
    let request = ResponseRequest::new("m", None, vec![]);
    let mut sink = KillAfterOne::default();
    let result = stream_turn(
        &reqwest::Client::new(),
        &provider_for(&base),
        &fast_requests(),
        &request,
        &mut sink,
    )
    .await
    .unwrap();
    assert_eq!(result.text, "abc");
    assert!(!result.completed);
    assert!(result.usage.is_none());
}

/// A sink that kills after the first event.
struct KillAfterOne {
    seen: u32,
}

impl KillAfterOne {
    const fn new() -> Self {
        Self { seen: 0 }
    }
}

impl Default for KillAfterOne {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnSink for KillAfterOne {
    fn event(&mut self, _: TurnEvent) -> bool {
        self.seen += 1;
        self.seen <= 1
    }
}

#[tokio::test]
async fn mid_body_drop_keeps_the_partial_turn() {
    // Server that sends one delta and drops the connection.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncWriteExt;
        let partial = "HTTP/1.1 200 OK
content-type: text/event-stream

data: "
            .to_string()
            + r#"{"type":"response.output_text.delta","delta":"survived"}"#
            + "

";
        let _ = sock.write_all(partial.as_bytes()).await;
        drop(sock);
    });
    let request = ResponseRequest::new("m", None, vec![]);
    struct Keep;
    impl TurnSink for Keep {
        fn event(&mut self, _: TurnEvent) -> bool {
            true
        }
    }
    let result = stream_turn(
        &reqwest::Client::new(),
        &provider_for(&format!("http://{addr}")),
        &fast_requests(),
        &request,
        &mut Keep,
    )
    .await
    .unwrap();
    assert_eq!(result.text, "survived");
    assert!(!result.completed, "an early EOF is an incomplete turn");
}
