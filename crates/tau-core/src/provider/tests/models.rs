use super::*;
use super::server::*;

#[tokio::test]
async fn list_models_success_parses_the_data_array() {
    let json = r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#;
    // A content-length: without it the body is read-to-close and the
    // mock's held-open socket stalls the client.
    let (base, _count) = mock_server(vec![format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
        json.len(),
        json
    )])
    .await;
    let client = test_client();
    let provider = provider_for(&base);
    let models = list_models(&client, &provider)
        .await
        .expect("the mock serves a 200");
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["m1", "m2"]
    );
}

#[tokio::test]
async fn list_models_non_success_is_a_status_error_carrying_the_body() {
    let body = "no such endpoint";
    let (base, _count) = mock_server(vec![format!(
        "HTTP/1.1 404 Not Found\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    )])
    .await;
    let client = test_client();
    let provider = provider_for(&base);
    let err = list_models(&client, &provider).await.unwrap_err();
    match err {
        ProviderError::Status { status, body } => {
            assert_eq!(status, 404);
            assert_eq!(body, "no such endpoint");
        }
        other => panic!("expected a Status error, got {other:?}"),
    }
}

#[tokio::test]
async fn list_models_connect_refusal_is_a_request_error() {
    let client = test_client();
    // Port 9 (discard) is closed: the send itself fails.
    let provider = provider_for("http://127.0.0.1:9/v1");
    let err = list_models(&client, &provider).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::Request(_)),
        "expected a Request error, got {err:?}"
    );
}

#[tokio::test]
async fn provider_error_display_names_each_variant() {
    // Request: a real connection refusal.
    let req = reqwest::get("http://127.0.0.1:9/v1/models")
        .await
        .err()
        .map(ProviderError::Request)
        .expect("the port is closed");
    assert!(req.to_string().contains("provider request failed"));

    assert!(ProviderError::IdleTimeout.to_string().contains("idle"));

    let status = ProviderError::Status {
        status: 500,
        body: "boom".into(),
    };
    assert_eq!(status.to_string(), "provider returned 500: boom");

    let malformed = ProviderError::MalformedStream("bad utf8".into());
    assert_eq!(malformed.to_string(), "malformed stream: bad utf8");
}

#[test]
fn resolve_key_reads_the_named_env_var_and_ignores_empty_values() {
    const VAR: &str = "TAU_TEST_RESOLVE_KEY";

    // No key env named: no key.
    let no_env = Provider {
        base_url: "http://x".into(),
        key_env: String::new(),
        models: std::collections::BTreeMap::new(),
    };
    assert!(resolve_key(&no_env).is_none());

    // Named and set: the value.
    unsafe { std::env::set_var(VAR, "secret") };
    let with_env = Provider {
        base_url: "http://x".into(),
        key_env: VAR.into(),
        models: std::collections::BTreeMap::new(),
    };
    assert_eq!(resolve_key(&with_env).as_deref(), Some("secret"));

    // Named but empty: treated as unset.
    unsafe { std::env::set_var(VAR, "") };
    assert!(resolve_key(&with_env).is_none());
    unsafe { std::env::remove_var(VAR) };
}
