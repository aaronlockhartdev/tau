//! OpenAI-compatible provider surface (spec §6): v0 talks to one API family —
//! the `responses` endpoint plus the plain REST endpoints like `/models` that
//! every OpenAI-compatible server implements. No provider detection, no
//! catalog, no keychain.

use crate::config::{Provider, Requests};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
pub use tau_protocol::payload::FunctionCall;
pub use tau_protocol::payload::TurnUsage as Usage;

/// A model as reported by the provider's `/models` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
}

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

impl std::error::Error for ProviderError {}

impl ProviderError {
    /// A 400 whose body carries a context-length signature (the #256
    /// classifier): the over-window rejections the bounded retry recovers
    /// from. Kept narrow — other 400s (auth, schema) are not over-window.
    #[must_use]
    pub fn is_context_length_error(&self) -> bool {
        let Self::Status { status, body } = self else {
            return false;
        };
        if *status != 400 {
            return false;
        }
        let lower = body.to_ascii_lowercase();
        lower.contains("context length")
            || lower.contains("too long")
            || lower.contains("input_tokens")
    }
}

impl fmt::Debug for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(e) => write!(f, "Request({e:?})"),
            Self::IdleTimeout => f.write_str("IdleTimeout"),
            Self::Status { status, body } => f
                .debug_struct("Status")
                .field("status", status)
                .field("body", body)
                .finish(),
            Self::MalformedStream(msg) => f.debug_tuple("MalformedStream").field(msg).finish(),
        }
    }
}

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
#[derive(Default)]
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

impl SseParser {
    #[must_use]
    pub const fn new() -> Self {
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
            let line = std::str::from_utf8(&self.buf[..=pos])
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

/// One block of a model turn, in the order the stream emitted it: the
/// JSONL response line's `blocks` (the #98 wire log's debug view). A model
/// turn interleaves reasoning, text, and tool calls; the flat `text` /
/// `reasoning` / `calls` fields are a lossy projection of this order.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireBlock {
    Reasoning {
        text: String,
    },
    Text {
        text: String,
    },
    /// `text` holds one JSON object per call, each on its own line.
    ToolCall {
        text: String,
    },
}

impl WireBlock {
    /// The block's consolidation class: runs of the same kind merge into
    /// one JSONL entry; a kind change starts a new one.
    fn kind(&self) -> &'static str {
        match self {
            Self::Reasoning { .. } => "reasoning",
            Self::Text { .. } => "text",
            Self::ToolCall { .. } => "tool_call",
        }
    }

    /// Append a same-kind block's payload: reasoning and text are plain
    /// concatenation; a tool call is serialized as a `name` / `arguments`
    /// object on a new line, so a merged run keeps each call distinct.
    fn append(&mut self, other: &Self) {
        match (self, other) {
            (Self::Reasoning { text: a }, Self::Reasoning { text: b })
            | (Self::Text { text: a }, Self::Text { text: b }) => {
                a.push_str(b);
            }
            (Self::ToolCall { text: a }, Self::ToolCall { text: b }) => {
                a.push('\n');
                a.push_str(b);
            }
            _ => {}
        }
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
    Created {
        #[serde(default)]
        created_at: Option<i64>,
    },
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
impl Frame {
    /// The wire-log blocks this frame represents, in arrival order: an
    /// `OutputTextDelta` is two dialects (the `reasoning_text` summary,
    /// then the `delta` text), the other deltas and `output_item.done`
    /// are one-for-one. A frame carrying none of them (completed, created,
    /// error, …) has no blocks.
    pub(crate) fn wire_blocks(&self) -> Vec<WireBlock> {
        match self {
            Self::OutputTextDelta {
                delta,
                reasoning_text,
                ..
            } => {
                let mut blocks = Vec::new();
                if let Some(text) = reasoning_text {
                    blocks.push(WireBlock::Reasoning { text: text.clone() });
                }
                if let Some(text) = delta {
                    blocks.push(WireBlock::Text { text: text.clone() });
                }
                blocks
            }
            Self::ReasoningDelta { delta: Some(text) }
            | Self::ReasoningTextDelta { delta: Some(text) } => {
                vec![WireBlock::Reasoning { text: text.clone() }]
            }
            Self::OutputItemDone {
                item:
                    OutputItem::FunctionCall {
                        name, arguments, ..
                    },
            } => vec![WireBlock::ToolCall {
                text: serde_json::json!({ "name": name, "arguments": arguments }).to_string(),
            }],
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CompletedResponse {
    #[serde(default)]
    usage: Option<usage::UsageWire>,
    /// #224: the server's completion timestamp — pairs with `Frame::Created`
    /// to yield the decode-window duration.
    #[serde(default)]
    created_at: Option<i64>,
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
    pub kind: String,
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// The assembled result of one turn.
#[derive(Debug, Default)]
pub struct TurnResult {
    pub text: String,
    pub reasoning: String,
    pub usage: Option<Usage>,
    /// #224: the server's decode-window duration
    /// (`completed.created_at − created.created_at`); `None` when the
    /// server omits either timestamp.
    pub duration: Option<Duration>,
    /// `response.completed` arrived; a kill or an early EOF leaves this
    /// false and the partial stands (spec §6/§7, #17 handoff gap b).
    pub completed: bool,
    pub calls: Vec<FunctionCall>,
    /// The turn's blocks in stream-arrival order (the JSONL response line's
    /// `blocks`): reasoning, text, and tool calls interleaved as the model
    /// emitted them, which the flat fields above lose.
    pub blocks: Vec<WireBlock>,
    /// Mid-stream provider error frames, in arrival order.
    pub mid_stream_errors: Vec<String>,
}

impl TurnResult {
    /// JSONL wire view of the response: one parseable JSON line — the
    /// model's exact output as ordered `blocks`, plus `ms`/`completed` /
    /// `usage` summary fields. `debug!` level (the #98 wire log's
    /// parseable companion) — compiled out in release builds. The verbatim
    /// SSE bytes stay at `trace` (`wire::response`).
    pub fn wire_response_jsonl(&self, id: &str, ms: u64) {
        tracing::debug!(
            line = %serde_json::json!({
                "id": id,
                "dir": "res",
                "ms": ms,
                "completed": self.completed,
                "usage": self.usage,
                "blocks": self.consolidated_blocks(),
            }),
            "llm wire"
        );
    }

    /// The blocks with consecutive same-kind fragments merged: a streamed
    /// turn arrives as dozens of `reasoning` / `text` / `tool_call` deltas,
    /// and the JSONL line reads far better as one entry per run (order
    /// preserved; the raw `self.blocks` stays as the stream emitted it).
    pub(crate) fn consolidated_blocks(&self) -> Vec<WireBlock> {
        let blocks = &self.blocks;
        let mut out: Vec<WireBlock> = Vec::with_capacity(blocks.len());
        for block in blocks {
            if let Some(last) = out.last_mut()
                && last.kind() == block.kind()
            {
                last.append(block);
                continue;
            }
            out.push(block.clone());
        }
        out
    }
}

/// One stream delta, or the usage at `response.completed`. The sink decides
/// whether the stream may continue (false = kill, spec §7).
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    Text(String),
    Reasoning(String),
    Completed(Usage, Option<Duration>),
}

/// A streaming sink: the force-kill point (spec §7).
pub trait TurnSink: Send {
    /// Return false to kill the in-flight stream; the partial result then
    /// stands with `completed: false`.
    fn event(&mut self, event: TurnEvent) -> bool;

    /// A future that completes when this sink wants the in-flight request
    /// torn down before the next stream event arrives (a stop during the
    /// prefill window, spec §7): the caller aborts the HTTP request and the
    /// partial stands, like a kill. Default: never.
    fn stop_signal(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}

pub fn fold_event(event: &TurnEvent, result: &mut TurnResult) {
    match event {
        TurnEvent::Text(t) => result.text.push_str(t),
        TurnEvent::Reasoning(t) => result.reasoning.push_str(t),
        TurnEvent::Completed(u, d) => {
            result.usage = Some(u.clone());
            if let Some(d) = d {
                result.duration = Some(*d);
            }
        }
    }
}

mod canned;
mod request;
mod stream;
mod usage;
mod wire;
pub use canned::*;
pub use request::*;
pub use stream::*;

#[cfg(test)]
mod sse_properties;
#[cfg(test)]
pub(crate) mod tests;
