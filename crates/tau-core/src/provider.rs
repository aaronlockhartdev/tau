//! OpenAI-compatible provider surface (spec §6): v0 talks to one API family —
//! the `responses` endpoint plus the plain REST endpoints like `/models` that
//! every OpenAI-compatible server implements. No provider detection, no
//! catalog, no keychain.

use crate::config::{Provider, Requests};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::time::Duration;

/// A model as reported by the provider's `/models` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
}

#[derive(Debug)]
pub enum ProviderError {
    Request(reqwest::Error),
    Status {
        status: u16,
        body: String,
    },
    /// A stream frame was not valid UTF-8 (spec §6 parser contract).
    MalformedStream(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(e) => write!(f, "provider request failed: {e}"),
            Self::Status { status, body } => write!(f, "provider returned {status}: {body}"),
            Self::MalformedStream(msg) => write!(f, "malformed stream: {msg}"),
        }
    }
}

impl Error for ProviderError {}

/// Join the API root with a sub-path, tolerating a trailing slash on `base`.
fn endpoint_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

fn resolve_key(provider: &Provider) -> Option<String> {
    if provider.key_env.is_empty() {
        return None;
    }
    std::env::var(&provider.key_env)
        .ok()
        .filter(|k| !k.is_empty())
}

/// `GET {base_url}/models` — the round-trip that proves provider wiring.
pub async fn list_models(
    client: &reqwest::Client,
    provider: &Provider,
) -> Result<Vec<Model>, ProviderError> {
    let mut request = client.get(endpoint_url(&provider.base_url, "models"));
    if let Some(key) = resolve_key(provider) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(ProviderError::Request)?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status, body });
    }
    let payload: ModelsResponse = response.json().await.map_err(ProviderError::Request)?;
    Ok(payload.data)
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<Model>,
}

/// In-tree SSE frame parser (spec §6, research #4): UTF-8-safe buffering,
/// blank-line event splitting, `data:` line concatenation, `:`-comment
/// skipping, `[DONE]` termination.
pub struct SseParser {
    buf: Vec<u8>,
    /// Set once a `[DONE]` data payload arrives; the caller stops reading.
    pub terminated: bool,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}
impl SseParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            terminated: false,
        }
    }

    /// Feed a raw chunk; returns the assembled data payloads of the events
    /// completed by this chunk (multiple `data:` lines joined with `\n`,
    /// per the SSE spec).
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderError> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut data: Vec<String> = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            // 0x0A never occurs inside a multi-byte UTF-8 sequence, so
            // cutting at a newline is UTF-8-safe; a partial code point at the
            // chunk edge simply stays buffered for the next feed.
            let line = std::str::from_utf8(&self.buf[..pos + 1])
                .map_err(|e| ProviderError::MalformedStream(e.to_string()))?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !data.is_empty() {
                    out.push(data.join("\n"));
                    data.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                data.push(value.strip_prefix(' ').unwrap_or(value).to_owned());
            }
            // `:`-prefixed lines are comments (OpenRouter keep-alives); the
            // other SSE fields (`event:`, `id:`, `retry:`) are unused by the
            // responses API and ignored.
            self.buf.drain(..=pos);
        }
        if !self.terminated && out.iter().any(|d| d == "[DONE]") {
            self.terminated = true;
        }
        Ok(out)
    }
}

/// One stream frame decoded from a `data:` payload. Servers deviate in field
/// names; the serde aliases fold the reasoning dialects (`reasoning` /
/// `reasoning_content` / `reasoning_details` / `thinking`) into one internal
/// field — the dialect mess is the client's problem (spec §6).
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Frame {
    #[serde(rename = "response.created")]
    Created,
    #[serde(rename = "response.in_progress")]
    InProgress,
    #[serde(rename = "response.completed")]
    Completed { response: Option<CompletedResponse> },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        delta: Option<String>,
        #[serde(
            alias = "reasoning",
            alias = "reasoning_content",
            alias = "thinking",
            default
        )]
        reasoning_text: Option<String>,
        #[serde(default)]
        reasoning_details: Option<Vec<ReasoningBlock>>,
    },
    /// OpenAI-native reasoning summary stream (`reasoning.summary` opt-in).
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningDelta {
        #[serde(default)]
        delta: Option<String>,
    },
    /// Mid-stream provider error: recorded, never fatal (spec §6 invariant).
    #[serde(rename = "error")]
    Error { error: ProviderErrorFrame },
    /// Lifecycle/content items the v0 client does not consume.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct CompletedResponse {
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct ReasoningBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderErrorFrame {
    #[serde(default)]
    message: Option<String>,
}

/// Usage from the `response.completed` event (spec §6). Field names accept
/// both the Responses shape and the chat-completions dialect.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    #[serde(alias = "prompt_tokens")]
    pub input_tokens: u64,
    #[serde(alias = "completion_tokens")]
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub output_tokens_details: Option<OutputTokensDetails>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputTokensDetails {
    pub reasoning_tokens: u64,
}

impl Usage {
    pub fn reasoning_tokens(&self) -> u64 {
        self.output_tokens_details
            .as_ref()
            .map(|d| d.reasoning_tokens)
            .unwrap_or(0)
    }
}

/// One request to `POST {base}/responses` (streaming; v0 is responses-only,
/// ADR-0003).
#[derive(Debug, Serialize)]
pub struct ResponseRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<InputMessage>,
    stream: bool,
}

impl ResponseRequest {
    pub fn new(
        model: impl Into<String>,
        instructions: Option<&str>,
        input: Vec<InputMessage>,
    ) -> Self {
        Self {
            model: model.into(),
            instructions: instructions.map(str::to_owned),
            input,
            stream: true,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct InputMessage {
    pub role: String,
    pub content: String,
}

/// The assembled result of one turn.
#[derive(Debug, Default)]
pub struct TurnResult {
    pub text: String,
    pub reasoning: String,
    pub usage: Option<Usage>,
    /// Mid-stream provider error frames, in arrival order.
    pub mid_stream_errors: Vec<String>,
}

fn append_reasoning(
    result: &mut TurnResult,
    field: Option<String>,
    blocks: Option<Vec<ReasoningBlock>>,
) {
    if let Some(text) = field {
        result.reasoning.push_str(&text);
    }
    if let Some(blocks) = blocks {
        for block in blocks {
            if let Some(text) = block.text {
                result.reasoning.push_str(&text);
            }
        }
    }
}

fn apply_frame(result: &mut TurnResult, payload: &str) {
    // Undecodable frames are skipped rather than failing the turn: servers
    // pad the stream with non-JSON frames (research #4, quirk 1).
    let frame: Frame = match serde_json::from_str(payload) {
        Ok(frame) => frame,
        Err(_) => return,
    };
    match frame {
        Frame::Completed { response } => {
            if let Some(usage) = response.and_then(|r| r.usage) {
                result.usage = Some(usage);
            }
        }
        Frame::OutputTextDelta {
            delta,
            reasoning_text,
            reasoning_details,
        } => {
            if let Some(text) = delta {
                result.text.push_str(&text);
            }
            append_reasoning(result, reasoning_text, reasoning_details);
        }
        Frame::ReasoningDelta { delta } => {
            if let Some(text) = delta {
                result.reasoning.push_str(&text);
            }
        }
        Frame::Error { error } => {
            result.mid_stream_errors.push(
                error
                    .message
                    .clone()
                    .unwrap_or_else(|| "mid-stream provider error".into()),
            );
        }
        Frame::Created | Frame::InProgress | Frame::Other => {}
    }
}

/// One streamed `POST {base}/responses` turn (spec §6): typed SSE frames,
/// reasoning-dialect normalization, usage from `response.completed`.
/// Retries connect/timeout/5xx up to `requests.retries` with exponential
/// backoff; 4xx and mid-stream errors fail or annotate the turn without
/// retrying.
pub async fn stream_response(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
    request: &ResponseRequest,
) -> Result<TurnResult, ProviderError> {
    let url = endpoint_url(&provider.base_url, "responses");
    let key = resolve_key(provider);
    let mut attempt = 0u32;
    loop {
        match attempt_one_turn(client, &url, key.as_deref(), requests, request).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let retryable = match &e {
                    ProviderError::Request(reqwest_err) => {
                        reqwest_err.is_connect() || reqwest_err.is_timeout()
                    }
                    ProviderError::Status { status, .. } => (500..599).contains(status),
                    _ => false,
                };
                if !retryable || attempt >= requests.retries {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(6))).await;
                attempt += 1;
            }
        }
    }
}

async fn attempt_one_turn(
    client: &reqwest::Client,
    url: &str,
    key: Option<&str>,
    requests: &Requests,
    request: &ResponseRequest,
) -> Result<TurnResult, ProviderError> {
    let mut builder = client
        .post(url)
        .timeout(Duration::from_secs(requests.timeout_secs))
        .json(request);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    let response = builder.send().await.map_err(ProviderError::Request)?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status, body });
    }
    let mut parser = SseParser::new();
    let mut result = TurnResult::default();
    let mut stream = response;
    'outer: while !parser.terminated {
        let Some(chunk) = stream.chunk().await.map_err(ProviderError::Request)? else {
            break;
        };
        for payload in parser.feed(&chunk)? {
            if payload == "[DONE]" {
                break 'outer;
            }
            apply_frame(&mut result, &payload);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

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
    fn usage_aliases_accept_chat_completions_names() {
        let u: Usage = serde_json::from_str(
            r#"{"prompt_tokens": 11, "completion_tokens": 5, "total_tokens": 16}"#,
        )
        .unwrap();
        assert_eq!(
            (u.input_tokens, u.output_tokens, u.total_tokens),
            (11, 5, 16)
        );
        let u: Usage = serde_json::from_str(
            r#"{"input_tokens": 3, "output_tokens": 9, "total_tokens": 12,
                "output_tokens_details": {"reasoning_tokens": 4}}"#,
        )
        .unwrap();
        assert_eq!(u.reasoning_tokens(), 4);
    }

    #[test]
    fn reasoning_dialects_normalize_into_one_field() {
        for field in ["reasoning", "reasoning_content", "thinking"] {
            let payload = format!(
                r#"{{"type":"response.output_text.delta","delta":"answer","{field}":"thought"}}"#
            );
            let mut result = TurnResult::default();
            apply_frame(&mut result, &payload);
            assert_eq!(result.text, "answer", "dialect {field}");
            assert_eq!(result.reasoning, "thought", "dialect {field}");
        }
        let payload = r#"{"type":"response.output_text.delta","delta":"answer",
            "reasoning_details": [{"text": "thought-a"}, {"text": "thought-b"}]}"#;
        let mut result = TurnResult::default();
        apply_frame(&mut result, payload);
        assert_eq!(result.reasoning, "thought-athought-b");
    }

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
            }],
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
            }],
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
            }],
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
        };
        let request = ResponseRequest::new(
            "m",
            None,
            vec![InputMessage {
                role: "user".into(),
                content: "hi".into(),
            }],
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
        let client = reqwest::Client::new();
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
        let client = reqwest::Client::new();
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
            }],
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
            ],
        );
        let turn2 = stream_response(&client, &provider, &requests, &follow_up)
            .await
            .expect("turn 2");
        assert!(!turn2.text.is_empty(), "turn 2 produced no text");
        assert!(turn2.usage.is_some(), "turn 2 recorded no usage");
    }
}
