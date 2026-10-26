//! The wire shape of one `POST {base}/responses` request (spec §6, #35):
//! the request body, its input items, and the soft prompt-size math the
//! output-cap clamp runs on.
use super::ToolSpec;
use crate::config::{CacheRetention, ThinkingLevel};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The reasoning effort in the responses-API shape (`{"effort": ...}`);
/// `None` leaves the server default (a thinking model reasons heavily by
/// default, which burns the output budget — spec §6).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

/// The config level → the wire effort (#35): `off` omits the parameter.
impl From<ThinkingLevel> for Option<ReasoningEffort> {
    fn from(level: ThinkingLevel) -> Self {
        match level {
            ThinkingLevel::Off => None,
            ThinkingLevel::Minimal => Some(ReasoningEffort::Minimal),
            ThinkingLevel::Low => Some(ReasoningEffort::Low),
            ThinkingLevel::Medium => Some(ReasoningEffort::Medium),
            ThinkingLevel::High => Some(ReasoningEffort::High),
            ThinkingLevel::XHigh => Some(ReasoningEffort::XHigh),
            ThinkingLevel::Max => Some(ReasoningEffort::Max),
        }
    }
}

/// The wire value for OpenAI-native reasoning summary (opt-in; streams as
/// `response.reasoning_summary_text.delta`).
pub(crate) const REASONING_SUMMARY: &str = "auto";

#[derive(Debug, Clone, Copy, Serialize)]
struct ReasoningParam {
    effort: ReasoningEffort,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'static str>,
}

/// The `prompt_cache_options` object (spec §12, #35).
#[derive(Debug, Clone, Copy, Serialize)]
struct PromptCacheOptions {
    #[serde(rename = "type")]
    kind: &'static str,
    retention: &'static str,
}

/// `cache.retention` → the wire's `retention` values (#35).
const fn cache_retention_wire(retention: CacheRetention) -> Option<&'static str> {
    match retention {
        CacheRetention::Short => Some("24h"),
        CacheRetention::Long => Some("7d"),
        CacheRetention::None => None,
    }
}

/// One request to `POST {base}/responses` (streaming; v0 is responses-only,
/// ADR-0003).
#[derive(Debug, Clone, Serialize)]
pub struct ResponseRequest {
    #[serde(skip)]
    wire_delta_from: usize,
    /// The caller's label for the wire log's request line (the OM
    /// round-trips' `observe`/`buffer`/`reflect`); `None` = the session's
    /// main turns. `serde(skip)`ed: log metadata, never wire content.
    #[serde(skip)]
    wire_label: Option<&'static str>,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<InputEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_options: Option<PromptCacheOptions>,
    stream: bool,
}

impl ResponseRequest {
    pub fn new(
        model: impl Into<String>,
        instructions: Option<&str>,
        input: Vec<InputEntry>,
    ) -> Self {
        Self {
            wire_delta_from: 0,
            wire_label: None,
            model: model.into(),
            instructions: instructions.map(str::to_owned),
            input,
            tools: Vec::new(),
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            reasoning: None,
            prompt_cache_key: None,
            prompt_cache_options: None,
            stream: true,
        }
    }

    /// The #98 wire log's delta view (agent-side seam): log only the input
    /// items from index `from` on — the ones appended since this session's
    /// previous wire request. The field is `serde(skip)`ed: it shapes the
    /// log line, never the wire body.
    #[must_use]
    pub(crate) fn with_wire_delta(mut self, from: usize) -> Self {
        self.wire_delta_from = from;
        self
    }

    /// The caller's label for the wire log's request line (the OM
    /// round-trips' `observe`/`buffer`/`reflect`).
    #[must_use]
    pub(crate) fn with_wire_label(mut self, label: &'static str) -> Self {
        self.wire_label = Some(label);
        self
    }

    /// JSONL wire view of this request: one parseable JSON line carrying the
    /// caller's label, the system prompt, and the *new* input items. The
    /// delta is the items appended since this session's previous request
    /// (`with_wire_delta`), further restricted to `function_call_output`
    /// items: the assistant message and the tool *calls* in the delta are a
    /// re-serialization of the model's own prior output, which the preceding
    /// `res` line already logged verbatim — echoing them here doubles the log
    /// with content that adds nothing. `debug!` level, so compiled out in
    /// release builds. Emitted once per attempt by the provider layer (which
    /// mints the id the response line pairs with); a retried attempt logs an
    /// empty delta, not the same items twice.
    pub fn wire_request_jsonl(&self, id: &str) {
        let delta: Vec<Value> = self
            .input
            .get(self.wire_delta_from..)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|e| match e {
                        InputEntry::CallOutput(o) => serde_json::to_value(o).ok(),
                        // The assistant message and the calls are the model's
                        // own prior output (already in the `res` line); the
                        // user message is static context. Neither is new.
                        InputEntry::Message(_) | InputEntry::Call(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        tracing::debug!(
            line = %serde_json::json!({
                "id": id,
                "dir": "req",
                "label": self.wire_label,
                "model": self.model,
                "instructions": self.instructions,
                "delta": delta,
                "msg_count": self.input.len(),
            }),
            "llm wire"
        );
    }

    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub const fn with_max_output_tokens(mut self, n: u64) -> Self {
        self.max_output_tokens = Some(n);
        self
    }

    #[must_use]
    pub const fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    #[must_use]
    pub const fn with_top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }

    #[must_use]
    pub const fn with_frequency_penalty(mut self, p: f32) -> Self {
        self.frequency_penalty = Some(p);
        self
    }

    #[must_use]
    pub const fn with_presence_penalty(mut self, p: f32) -> Self {
        self.presence_penalty = Some(p);
        self
    }

    #[must_use]
    pub const fn with_reasoning(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning = Some(ReasoningParam {
            effort,
            summary: None,
        });
        self
    }

    /// `thinking.summary` (spec §12, #35): the summary streams as its own
    /// delta dialect alongside the text.
    #[must_use]
    pub const fn with_reasoning_summary(mut self) -> Self {
        if let Some(param) = &mut self.reasoning {
            param.summary = Some(REASONING_SUMMARY);
        }
        self
    }

    /// `cache.retention` (spec §12, #35): the key is the session's stable
    /// cache identity; `none` emits nothing.
    #[must_use]
    pub fn with_prompt_cache(mut self, key: String, retention: CacheRetention) -> Self {
        self.prompt_cache_key = Some(key);
        if let Some(retention) = cache_retention_wire(retention) {
            self.prompt_cache_options = Some(PromptCacheOptions {
                kind: "default",
                retention,
            });
        }
        self
    }

    /// The caller's system-prompt instructions (test seam: the OM async
    /// tests capture them to assert what the next main-turn call assembles).
    #[must_use]
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// The request's input entries (test seam: the OM async tests capture
    /// them to assert the assembled raw window).
    #[must_use]
    pub fn input(&self) -> &[InputEntry] {
        &self.input
    }

    /// The request's output cap (exposed for the recovery tests to capture
    /// it and assert the halving).
    #[must_use]
    pub fn max_output_tokens(&self) -> Option<u64> {
        self.max_output_tokens
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
/// the base64 payload as a data URL (#34). A downscaled image (#257) adds
/// its coordinate-mapping note as a leading `input_text` part.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CallOutput {
    Text(String),
    Image(Vec<InputCallPart>),
}

impl CallOutput {
    /// A stored tool output as wire content (#34): a string passes through;
    /// an image block becomes one `input_image` data-URL part, preceded by an
    /// `input_text` part when the block carries a downscale note (#257).
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
                let mut parts = Vec::new();
                if let Some(note) = obj.get("note").and_then(Value::as_str) {
                    parts.push(InputCallPart::Text(note.to_owned()));
                }
                parts.push(InputCallPart::Image(InputImagePart {
                    kind: "input_image",
                    image_url: format!("data:{media_type};base64,{data}"),
                    detail: "auto",
                }));
                Some(Self::Image(parts))
            }
            _ => None,
        }
    }
}

/// A content part of a `function_call_output.output` array: the image, or
/// the downscale note that precedes it (#257).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum InputCallPart {
    Text(String),
    Image(InputImagePart),
}
/// An `input_image` content part (responses API): the base64 payload in a
/// data URL; `detail` is always "auto" (vLLM's Responses model requires the
/// field, `OpenAI`'s defaults it) (#79).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InputImagePart {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub image_url: String,
    pub detail: &'static str,
}

/// Soft prompt size in tokens (the shared bytes/4 estimator, spec §4): the
/// system prompt, every input item, and the tool specs as serialized, with the
/// serialization allowance on top (plan §6.4).
pub(crate) fn prompt_token_estimate(
    system_prompt: &str,
    input: &[InputEntry],
    tools_json: &str,
) -> u32 {
    let mut n = crate::om::token_count(system_prompt);
    n = n.saturating_add(items_estimate(input));
    n = n.saturating_add(crate::om::token_count(tools_json));
    with_serialization_allowance(n)
}

/// The estimate for input items alone — the whole conversation when no anchor
/// covers it, one tool output when one does (plan §6.4: the provider's count is
/// the authority for what was already sent, so only the delta is ever a guess).
/// It carries its own framing allowance, since a delta rides a request whose
/// rest is already counted.
pub(crate) fn items_token_estimate(items: &[InputEntry]) -> u32 {
    with_serialization_allowance(items_estimate(items))
}

fn items_estimate(items: &[InputEntry]) -> u32 {
    items
        .iter()
        .fold(0u32, |n, item| n.saturating_add(item_token_estimate(item)))
}

/// JSON framing a content estimate never sees — the 20% the comparable
/// harnesses add for serialization overhead (plan §6.4: Codex's
/// `with_serialization_allowance`).
fn with_serialization_allowance(estimate: u32) -> u32 {
    estimate.saturating_add(estimate / 5)
}

/// One input item's content, the shape the request actually carries it in.
fn item_token_estimate(item: &InputEntry) -> u32 {
    match item {
        InputEntry::Message(m) => crate::om::token_count(&m.content),
        InputEntry::Call(c) => crate::om::token_count(&c.arguments),
        InputEntry::CallOutput(o) => match &o.output {
            CallOutput::Text(s) => crate::om::token_count(s),
            // #255: an image part's base64 data URL carries the payload
            // (the #34 shape) — counting it keeps the gate's budget math
            // honest, so an over-window image prompt is caught pre-wire.
            CallOutput::Image(parts) => parts.iter().fold(0u32, |n, p| {
                n.saturating_add(match p {
                    InputCallPart::Text(t) => crate::om::token_count(t),
                    InputCallPart::Image(img) => crate::om::token_count(&img.image_url),
                })
            }),
        },
    }
}

/// The output cap clamped to a model's declared context window
/// (`context_window` − prompt; spec §12, #35). A prompt that already fills
/// the window — or an undeclared window — leaves the cap to the server's
/// own rejection.
pub(crate) fn clamp_max_output(max: u64, context_window: Option<u32>, prompt_tokens: u32) -> u64 {
    match context_window {
        Some(window) if u64::from(window) > u64::from(prompt_tokens) => {
            max.min(u64::from(window) - u64::from(prompt_tokens))
        }
        _ => max,
    }
}
