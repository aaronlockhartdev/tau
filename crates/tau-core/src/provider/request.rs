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

/// OpenAI-native reasoning summary (opt-in; streams as
/// `response.reasoning_summary_text.delta`).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ReasoningSummary {
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ReasoningParam {
    effort: ReasoningEffort,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<ReasoningSummary>,
}

/// The `prompt_cache_options` object (spec §12, #35).
#[derive(Debug, Clone, Copy, Serialize)]
struct PromptCacheOptions {
    #[serde(rename = "type")]
    kind: &'static str,
    retention: &'static str,
}

/// `cache.retention` → the wire's `retention` values (#35).
fn cache_retention_wire(retention: CacheRetention) -> Option<&'static str> {
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

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_max_output_tokens(mut self, n: u64) -> Self {
        self.max_output_tokens = Some(n);
        self
    }

    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }

    pub fn with_frequency_penalty(mut self, p: f32) -> Self {
        self.frequency_penalty = Some(p);
        self
    }

    pub fn with_presence_penalty(mut self, p: f32) -> Self {
        self.presence_penalty = Some(p);
        self
    }

    pub fn with_reasoning(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning = Some(ReasoningParam {
            effort,
            summary: None,
        });
        self
    }

    /// `thinking.summary` (spec §12, #35): the summary streams as its own
    /// delta dialect alongside the text.
    pub fn with_reasoning_summary(mut self) -> Self {
        if let Some(param) = &mut self.reasoning {
            param.summary = Some(ReasoningSummary::Auto);
        }
        self
    }

    /// `cache.retention` (spec §12, #35): the key is the session's stable
    /// cache identity; `none` emits nothing.
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

/// Soft prompt size in tokens (the shared ~4-chars/token estimator, spec
/// §4): the system prompt, every input item, and the serialized tool specs.
pub(crate) fn prompt_token_estimate(
    system_prompt: &str,
    input: &[InputEntry],
    tools: &[ToolSpec],
) -> u32 {
    let mut n = crate::om::token_count(system_prompt);
    for entry in input {
        n += match entry {
            InputEntry::Message(m) => crate::om::token_count(&m.content),
            InputEntry::Call(c) => crate::om::token_count(&c.arguments),
            InputEntry::CallOutput(o) => match &o.output {
                CallOutput::Text(s) => crate::om::token_count(s),
                CallOutput::Image(_) => 0,
            },
        };
    }
    if let Ok(json) = serde_json::to_string(tools) {
        n += crate::om::token_count(&json);
    }
    n
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
