use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
fn ok_body(frames: &[&str]) -> String {
    let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    for frame in frames {
        body.push_str(&sse(frame));
    }
    body
}

/// One-shot HTTP server on 127.0.0.1:0; each connection gets the canned
/// body from `bodies` (cycling) and the request is counted.
async fn mock_server(bodies: Vec<String>) -> (String, Arc<AtomicU32>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicU32::new(0));
    let count2 = count.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => break,
            };
            count2.fetch_add(1, Ordering::SeqCst);
            let body =
                bodies[(count2.load(Ordering::SeqCst) - 1) as usize % bodies.len()].clone();
            use tokio::io::AsyncWriteExt;
            let _ = sock.write_all(body.as_bytes()).await;
        }
    });
    (format!("http://{addr}"), count)
}

fn provider_for(base: &str) -> Provider {
    Provider {
        base_url: base.into(),
        key_env: String::new(),
        models: Vec::new(),
    }
}

fn fast_requests() -> Requests {
    Requests {
        timeout_secs: 10,
        retries: 2,
        tool_batch_on_force: crate::config::ToolBatchPolicy::default(),
    }
}

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
    assert_eq!(result.usage.unwrap().total_tokens, 3);
}

#[tokio::test]
async fn server_5xx_is_retried_then_succeeds() {
    let (base, count) = mock_server(vec![
        "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 2\r\n\r\nxx".into(),
        "HTTP/1.1 502 Bad Gateway\r\ncontent-length: 2\r\n\r\nyy".into(),
        ok_body(&[
            r#"{"type":"response.output_text.delta","delta":"ok"}"#,
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
            "[DONE]",
        ]),
    ])
    .await;
    let requests = fast_requests();
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
        &requests,
        &request,
    )
    .await
    .unwrap();
    assert_eq!(result.text, "ok");
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn server_4xx_fails_without_retry() {
    let (base, count) = mock_server(vec![
        "HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\n\r\nno".into(),
    ])
    .await;
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
    let err = stream_response(
        &reqwest::Client::new(),
        &provider_for(&base),
        &fast_requests(),
        &request,
    )
    .await
    .unwrap_err();
    match err {
        ProviderError::Status { status, .. } => assert_eq!(status, 401),
        other => panic!("expected Status, got {other:?}"),
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn timeout_is_retried_and_surfaced() {
    // Server that accepts and never answers.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicU32::new(0));
    let count2 = count.clone();
    tokio::spawn(async move {
        loop {
            let (sock, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => break,
            };
            count2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                drop(sock);
            });
        }
    });
    let requests = Requests {
        timeout_secs: 1,
        retries: 1,
        tool_batch_on_force: crate::config::ToolBatchPolicy::default(),
    };
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
    let err = stream_response(
        &reqwest::Client::new(),
        &provider_for(&format!("http://{addr}")),
        &requests,
        &request,
    )
    .await
    .unwrap_err();
    match err {
        ProviderError::Request(e) => assert!(e.is_timeout(), "expected timeout, got {e}"),
        other => panic!("expected Request(timeout), got {other:?}"),
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

/// Live round-trip against a local OpenAI-compatible server; skipped unless
/// TAU_TEST_ENDPOINT is set (CI has no model server).
#[tokio::test]
async fn models_roundtrip() {
    let base = match std::env::var("TAU_TEST_ENDPOINT") {
        Ok(base) => base,
        Err(_) => return,
    };
    let client = test_client();
    let provider = Provider {
        base_url: base,
        key_env: String::new(),
        models: Vec::new(),
    };
    let models = list_models(&client, &provider)
        .await
        .expect("models round-trip");
    assert!(!models.is_empty());
}

/// Multi-message conversation against the local rapid-mlx endpoint
/// (ticket #17 acceptance); skipped unless TAU_TEST_ENDPOINT is set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_message_conversation_streams() {
    let base = match std::env::var("TAU_TEST_ENDPOINT") {
        Ok(base) => base,
        Err(_) => return,
    };
    let client = test_client();
    let provider = Provider {
        base_url: base,
        key_env: String::new(),
        models: vec!["qwen3.5-9b-4bit".into()],
    };
    let requests = Requests::default();
    let request = ResponseRequest::new(
        "qwen3.5-9b-4bit",
        Some("Answer in one short line."),
        vec![InputMessage {
            role: "user".into(),
            content: "What is 27 times 4? Reply with just the number.".into(),
        }]
        .into_iter()
        .map(InputEntry::Message)
        .collect(),
    );
    let turn1 = stream_response(&client, &provider, &requests, &request)
        .await
        .expect("turn 1");
    assert!(!turn1.text.is_empty(), "turn 1 produced no text");
    assert!(turn1.usage.is_some(), "turn 1 recorded no usage");
    // The conversation continues: the second request carries the first
    // exchange and asks a follow-up.
    let follow_up = ResponseRequest::new(
        "qwen3.5-9b-4bit",
        Some("Answer in one short line."),
        vec![
            InputMessage {
                role: "user".into(),
                content: "What is 27 times 4? Reply with just the number.".into(),
            },
            InputMessage {
                role: "assistant".into(),
                content: turn1.text.clone(),
            },
            InputMessage {
                role: "user".into(),
                content: "Now double that number.".into(),
            },
        ]
        .into_iter()
        .map(InputEntry::Message)
        .collect(),
    );
    let turn2 = stream_response(&client, &provider, &requests, &follow_up)
        .await
        .expect("turn 2");
    assert!(!turn2.text.is_empty(), "turn 2 produced no text");
    assert!(turn2.usage.is_some(), "turn 2 recorded no usage");
}

