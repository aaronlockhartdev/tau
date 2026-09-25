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
pub use tau_protocol::payload::TurnUsage as Usage;
pub use tau_protocol::payload::{
    FunctionCall, OutputTokensDetails, PromptTokensDetails, TurnUsage,
};

/// A model as reported by the provider's `/models` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
}

#[derive(Debug)]
pub enum ProviderError {
    Request(reqwest::Error),
    /// No data for `requests.timeout_secs` (connect, headers, or
    /// between chunks): a dead stream, cut by the per-chunk idle deadline.
    IdleTimeout,
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
            Self::IdleTimeout => write!(
                f,
                "provider stream went idle (no data for the idle timeout)"
            ),
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
    /// The `data:` payloads of the in-flight event; a blank line completes
    /// it. Parser state, not a `feed` local: a `data:` line and its
    /// terminating blank line can split across chunks, and a lost payload
    /// here is a lost event (the property test guards this).
    data: Vec<String>,
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
            data: Vec::new(),
            terminated: false,
        }
    }

    /// Feed a raw chunk; returns the assembled data payloads of the events
    /// completed by this chunk (multiple `data:` lines joined with `\n`,
    /// per the SSE spec).
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderError> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            // 0x0A never occurs inside a multi-byte UTF-8 sequence, so
            // cutting at a newline is UTF-8-safe; a partial code point at the
            // chunk edge simply stays buffered for the next feed.
            let line = std::str::from_utf8(&self.buf[..pos + 1])
                .map_err(|e| ProviderError::MalformedStream(e.to_string()))?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
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
    pub output: CallOutput,
}

/// The tool result as the responses API carries it: a plain JSON string
/// (the common path), or a content-parts array. The API accepts both for
/// `function_call_output.output`; an image is an `input_image` part with
/// the base64 payload as a data URL (#34).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CallOutput {
    Text(String),
    Image(Vec<InputImagePart>),
}

impl CallOutput {
    /// A stored tool output as wire content (#34): a string passes through;
    /// an image block becomes one `input_image` data-URL part.
    pub fn from_payload(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Some(Self::Text(s.clone())),
            obj if obj.get("type").and_then(Value::as_str) == Some("image") => {
                let media_type = obj
                    .get("media_type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let data = obj
                    .get("data_base64")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Some(Self::Image(vec![InputImagePart {
                    kind: "input_image",
                    image_url: format!("data:{media_type};base64,{data}"),
                }]))
            }
            _ => None,
        }
    }
}

/// An `input_image` content part (responses API): the base64 payload in a
/// data URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InputImagePart {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub image_url: String,
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

mod canned;
mod stream;
pub use canned::*;
pub use stream::*;

#[cfg(test)]
mod sse_properties;
#[cfg(test)]
pub(crate) mod tests;
