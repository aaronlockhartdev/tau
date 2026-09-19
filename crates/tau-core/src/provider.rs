//! OpenAI-compatible provider surface (spec §6): v0 talks to one API family —
//! the `responses` endpoint plus the plain REST endpoints like `/models` that
//! every OpenAI-compatible server implements. No provider detection, no
//! catalog, no keychain.

use crate::config::{Provider, Requests};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
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
    /// A completed output item — the carrier for a function call (spec §5.4).
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        #[serde(default)]
        item: OutputItem,
    },
    /// vLLM's native reasoning-delta dialect (`reasoning_text` parts on the
    /// reasoning item); folded into the one internal field like the rest.
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        #[serde(default)]
        delta: Option<String>,
    },
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

/// A completed `response.output_item.done` item. Only the function-call
/// shape is consumed (spec §5.4); the rest is ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputItem {
    FunctionCall {
        #[serde(default)]
        id: String,
        #[serde(default)]
        call_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        arguments: String,
    },
    #[default]
    #[serde(other)]
    Other,
}

/// One model-facing tool definition, wire-shaped for the responses API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: ToolKind,
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Function,
}

/// The reasoning budget in the responses-API shape (`{"effort": ...}`);
/// `None` leaves the server default (a thinking model reasons heavily by
/// default, which burns the output budget — spec §6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct ReasoningParam {
    effort: ReasoningEffort,
}

/// A function call the model emitted on a turn (spec §5.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunctionCall {
    pub id: String,
    pub call_id: String,
    pub name: String,
    /// Raw JSON, as the server sent it.
    pub arguments: String,
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
    #[serde(alias = "input_tokens_details")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

/// Prompt-side details: how many prompt tokens the server served from its
/// prefix cache (vLLM/OpenAI `prompt_tokens_details.cached_tokens`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptTokensDetails {
    pub cached_tokens: u64,
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
#[derive(Debug, Clone, Serialize)]
pub struct ResponseRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<InputEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
    stream: bool,
}

impl ResponseRequest {
    pub fn new(
        model: impl Into<String>,
        instructions: Option<&str>,
        input: Vec<InputEntry>,
    ) -> Self {
        Self {
            model: model.into(),
            instructions: instructions.map(str::to_owned),
            input,
            tools: Vec::new(),
            max_output_tokens: None,
            reasoning: None,
            stream: true,
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_max_output_tokens(mut self, n: u64) -> Self {
        self.max_output_tokens = Some(n);
        self
    }

    pub fn with_reasoning(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning = Some(ReasoningParam { effort });
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InputMessage {
    pub role: String,
    pub content: String,
}

/// One input item, wire-shaped: a plain message, a prior function call, or a
/// tool result (spec §5.4: results ride back to the model on the next call).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum InputEntry {
    Message(InputMessage),
    Call(FunctionCallInput),
    CallOutput(FunctionCallOutputInput),
}

/// A prior function call as an input item (responses API shape).
#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallInput {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// A tool result as an input item (responses API shape).
#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallOutputInput {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub call_id: String,
    pub output: String,
}
/// The assembled result of one turn.
#[derive(Debug, Default)]
pub struct TurnResult {
    pub text: String,
    pub reasoning: String,
    pub usage: Option<Usage>,
    /// `response.completed` arrived; a kill or an early EOF leaves this
    /// false and the partial stands (spec §6/§7, #17 handoff gap b).
    pub completed: bool,
    pub calls: Vec<FunctionCall>,
    /// Mid-stream provider error frames, in arrival order.
    pub mid_stream_errors: Vec<String>,
}

/// One stream delta, or the usage at `response.completed`. The sink decides
/// whether the stream may continue (false = kill, spec §7).
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    Text(String),
    Reasoning(String),
    Completed(Usage),
}

/// A streaming sink: the force-kill point (spec §7).
pub trait TurnSink: Send {
    /// Return false to kill the in-flight stream; the partial result then
    /// stands with `completed: false`.
    fn event(&mut self, event: TurnEvent) -> bool;
}

pub fn fold_event(event: &TurnEvent, result: &mut TurnResult) {
    match event {
        TurnEvent::Text(t) => result.text.push_str(t),
        TurnEvent::Reasoning(t) => result.reasoning.push_str(t),
        TurnEvent::Completed(u) => result.usage = Some(u.clone()),
    }
}

fn emit(sink: &mut dyn TurnSink, result: &mut TurnResult, event: TurnEvent) -> bool {
    if !sink.event(event.clone()) {
        return false;
    }
    fold_event(&event, result);
    true
}

/// Consume one `data:` payload: accumulate into the result and forward the
/// stream-delta events to the sink (false = kill, spec §7).
fn apply_frame(sink: &mut dyn TurnSink, result: &mut TurnResult, payload: &str) -> bool {
    // Undecodable frames are skipped rather than failing the turn: servers
    // pad the stream with non-JSON frames (research #4, quirk 1).
    let frame: Frame = match serde_json::from_str(payload) {
        Ok(frame) => frame,
        Err(_) => return true,
    };
    match frame {
        Frame::Completed { response } => {
            result.completed = true;
            match response.and_then(|r| r.usage) {
                Some(usage) => emit(sink, result, TurnEvent::Completed(usage)),
                None => true,
            }
        }
        Frame::OutputTextDelta {
            delta,
            reasoning_text,
            reasoning_details,
        } => {
            if let Some(text) = delta
                && !emit(sink, result, TurnEvent::Text(text))
            {
                return false;
            }
            if let Some(text) = reasoning_text
                && !emit(sink, result, TurnEvent::Reasoning(text))
            {
                return false;
            }
            for block in reasoning_details.into_iter().flatten() {
                if let Some(text) = block.text
                    && !emit(sink, result, TurnEvent::Reasoning(text))
                {
                    return false;
                }
            }
            true
        }
        Frame::ReasoningDelta { delta } | Frame::ReasoningTextDelta { delta } => {
            if let Some(text) = delta {
                emit(sink, result, TurnEvent::Reasoning(text))
            } else {
                true
            }
        }
        Frame::OutputItemDone { item } => {
            if let OutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } = item
            {
                result.calls.push(FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments,
                });
            }
            true
        }
        Frame::Error { error } => {
            result.mid_stream_errors.push(
                error
                    .message
                    .clone()
                    .unwrap_or_else(|| "mid-stream provider error".into()),
            );
            true
        }
        Frame::Created | Frame::InProgress | Frame::Other => true,
    }
}

/// The loop's provider seam (spec §6): a turn is a request in, a result out,
/// with every stream delta forwarded to the sink on the way — the live path
/// and the canned test path share this contract (ticket #19).
pub type ProviderTurn<'a> =
    Pin<Box<dyn Future<Output = Result<TurnResult, ProviderError>> + Send + 'a>>;

pub trait TurnProvider: Send + Sync {
    fn call<'a>(&self, request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a>;
}

pub type TurnProviderRef = Arc<dyn TurnProvider>;

/// The production seam: the live responses endpoint (spec §6).
pub fn production(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
) -> TurnProviderRef {
    Arc::new(ProductionProvider {
        client: client.clone(),
        provider: provider.clone(),
        requests: requests.clone(),
    })
}

struct ProductionProvider {
    client: reqwest::Client,
    provider: Provider,
    requests: Requests,
}

impl TurnProvider for ProductionProvider {
    fn call<'a>(&self, request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let client = self.client.clone();
        let provider = self.provider.clone();
        let requests = self.requests.clone();
        let request = request.clone();
        Box::pin(async move { stream_turn(&client, &provider, &requests, &request, sink).await })
    }
}

/// A canned provider for tests (ticket #19's SSE-fixture seam): replays the
/// events decoded from a canned SSE body through the same sink contract as
/// the live path, so lane semantics run against the real decode pipeline.
struct CannedProvider {
    events: Vec<TurnEvent>,
    /// Calls indexed by the event position they completed at.
    calls: Vec<(usize, FunctionCall)>,
    /// Cut the stream after this many events (the force-kill shape);
    /// `usize::MAX` = the full stream.
    cut_after: usize,
}

impl TurnProvider for CannedProvider {
    fn call<'a>(&self, _request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let events = self.events.clone();
        let calls = self.calls.clone();
        let cut_after = self.cut_after;
        Box::pin(async move {
            let mut result = TurnResult::default();
            let mut accepted = 0usize;
            for event in &events {
                if accepted >= cut_after {
                    break;
                }
                if !sink.event(event.clone()) {
                    break;
                }
                fold_event(event, &mut result);
                accepted += 1;
            }
            if accepted == events.len() {
                result.completed = events.iter().any(|e| matches!(e, TurnEvent::Completed(_)));
            }
            result.calls = calls
                .iter()
                .filter(|(i, _)| *i <= accepted)
                .map(|(_, c)| c.clone())
                .collect();
            Ok(result)
        })
    }
}

/// A canned provider replaying the full decoded stream.
pub fn canned(body: &str) -> TurnProviderRef {
    let (events, calls) = decode_stream(body).expect("canned SSE body must decode");
    Arc::new(CannedProvider {
        events,
        calls,
        cut_after: usize::MAX,
    })
}

/// A canned provider whose stream is cut after `n` events — the shape a
/// force-kill produces (partial, `completed: false`).
pub fn canned_cut(body: &str, n: usize) -> TurnProviderRef {
    let (events, calls) = decode_stream(body).expect("canned SSE body must decode");
    Arc::new(CannedProvider {
        events,
        calls,
        cut_after: n,
    })
}

/// A canned provider that sleeps between events — the in-flight seam for
/// force-kill tests: the loop's kill flag, not a pre-cut, terminates it.
struct SlowCannedProvider {
    events: Vec<TurnEvent>,
    delay_ms: u64,
}

impl TurnProvider for SlowCannedProvider {
    fn call<'a>(&self, _request: &ResponseRequest, sink: &'a mut dyn TurnSink) -> ProviderTurn<'a> {
        let events = self.events.clone();
        let delay_ms = self.delay_ms;
        Box::pin(async move {
            let mut result = TurnResult::default();
            let mut accepted = 0usize;
            for event in &events {
                if !sink.event(event.clone()) {
                    break;
                }
                fold_event(event, &mut result);
                accepted += 1;
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            if accepted == events.len() {
                result.completed = events.iter().any(|e| matches!(e, TurnEvent::Completed(_)));
            }
            Ok(result)
        })
    }
}

/// A canned provider replaying its stream with a delay between events: a
/// force sent mid-stream cuts it (the loop's kill flag is the terminator).
pub fn canned_slow(body: &str, delay_ms: u64) -> TurnProviderRef {
    let (events, _calls) = decode_stream(body).expect("canned SSE body must decode");
    Arc::new(SlowCannedProvider { events, delay_ms })
}

/// Decode a canned SSE body into the event/call sequence the live path would
/// produce (the canned provider's source; also the offline-decode utility).
/// A canned stream: the events in order, plus each call with the event
/// position it completed at.
pub type CannedStream = (Vec<TurnEvent>, Vec<(usize, FunctionCall)>);

pub fn decode_stream(body: &str) -> Result<CannedStream, ProviderError> {
    let mut parser = SseParser::new();
    let payloads = parser.feed(body.as_bytes())?;
    struct RecordSink(Vec<TurnEvent>);
    impl TurnSink for RecordSink {
        fn event(&mut self, event: TurnEvent) -> bool {
            self.0.push(event);
            true
        }
    }
    let mut sink = RecordSink(Vec::new());
    let mut result = TurnResult::default();
    let mut calls = Vec::new();
    let mut seen_calls = 0usize;
    for payload in payloads {
        if payload == "[DONE]" {
            break;
        }
        apply_frame(&mut sink, &mut result, &payload);
        while result.calls.len() > seen_calls {
            let call = result
                .calls
                .last()
                .cloned()
                .expect("calls appended in order");
            calls.push((sink.0.len(), call));
            seen_calls += 1;
        }
    }
    Ok((sink.0, calls))
}

/// One streamed `POST {base}/responses` turn (spec §6): typed SSE frames,
/// reasoning-dialect normalization, usage from `response.completed`, and the
/// delta events on the sink. Retries connect/timeout/5xx up to
/// `requests.retries` with exponential backoff; 4xx and mid-stream errors
/// fail or annotate the turn without retrying.
pub async fn stream_turn(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
    request: &ResponseRequest,
    sink: &mut dyn TurnSink,
) -> Result<TurnResult, ProviderError> {
    let url = endpoint_url(&provider.base_url, "responses");
    let key = resolve_key(provider);
    let mut attempt = 0u32;
    loop {
        match attempt_one_turn(client, &url, key.as_deref(), requests, request, sink).await {
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

/// One streamed turn; the events are discarded (the original #17 surface).
pub async fn stream_response(
    client: &reqwest::Client,
    provider: &Provider,
    requests: &Requests,
    request: &ResponseRequest,
) -> Result<TurnResult, ProviderError> {
    struct Keep;
    impl TurnSink for Keep {
        fn event(&mut self, _: TurnEvent) -> bool {
            true
        }
    }
    stream_turn(client, provider, requests, request, &mut Keep).await
}

async fn attempt_one_turn(
    client: &reqwest::Client,
    url: &str,
    key: Option<&str>,
    requests: &Requests,
    request: &ResponseRequest,
    sink: &mut dyn TurnSink,
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
        let chunk = match stream.chunk().await {
            Ok(Some(chunk)) => chunk,
            // EOF: the result stands as accumulated — `completed` only if
            // `response.completed` arrived (spec §6, #17 handoff gap b).
            Ok(None) => break,
            Err(e) => {
                // A mid-body drop keeps the partial as an incomplete turn
                // instead of an error (ticket #19, #17 handoff gap c): the
                // partial cannot be re-derived by a retry; a pre-body
                // failure (nothing received) still fails and retries.
                if result.text.is_empty() && result.reasoning.is_empty() && result.calls.is_empty()
                {
                    return Err(ProviderError::Request(e));
                }
                break;
            }
        };
        for payload in parser.feed(&chunk)? {
            if payload == "[DONE]" {
                break 'outer;
            }
            if !apply_frame(sink, &mut result, &payload) {
                // Killed: the partial stands (spec §7 force).
                return Ok(result);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

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
            apply_frame(&mut Keep, &mut result, &payload);
            assert_eq!(result.text, "answer", "dialect {field}");
            assert_eq!(result.reasoning, "thought", "dialect {field}");
        }
        let payload = r#"{"type":"response.output_text.delta","delta":"answer",
            "reasoning_details": [{"text": "thought-a"}, {"text": "thought-b"}]}"#;
        let mut result = TurnResult::default();
        apply_frame(&mut Keep, &mut result, payload);
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

    #[test]
    fn request_wire_shape_carries_tools_and_limits() {
        let request = ResponseRequest::new(
            "m",
            Some("sys"),
            vec![InputMessage {
                role: "user".into(),
                content: "hi".into(),
            }]
            .into_iter()
            .map(InputEntry::Message)
            .collect(),
        )
        .with_tools(vec![ToolSpec {
            kind: ToolKind::Function,
            name: "bash".into(),
            description: "run".into(),
            parameters: serde_json::json!({"type": "object"}),
        }])
        .with_max_output_tokens(128)
        .with_reasoning(ReasoningEffort::Low);
        let wire: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(wire["stream"], true);
        assert_eq!(wire["tools"][0]["type"], "function");
        assert_eq!(wire["tools"][0]["name"], "bash");
        assert_eq!(wire["max_output_tokens"], 128);
        assert_eq!(wire["reasoning"]["effort"], "low");
        assert_eq!(wire["input"][0]["role"], "user");
    }
}
