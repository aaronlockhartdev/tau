//! The entry payload surface (C9): one owner for the shape each entry
//! kind carries across the core↔GUI seam. The session file stays
//! free-form at the ADR-0005 storage level; the types here are what the
//! writers construct and what the projections carry (the snapshot and
//! `LiveState` shapes are built from them, never from raw maps).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A function call the model emitted on a turn (spec §5.4).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub id: String,
    pub call_id: String,
    pub name: String,
    /// Raw JSON, as the server sent it.
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Active,
    Done,
    Skipped,
}

impl StepStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Active => "active",
            StepStatus::Done => "done",
            StepStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub text: String,
    pub expected_output: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionStatus {
    Pending,
    Satisfied,
    Failed,
    Skipped,
}

impl CriterionStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            CriterionStatus::Pending => "pending",
            CriterionStatus::Satisfied => "satisfied",
            CriterionStatus::Failed => "failed",
            CriterionStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    pub text: String,
    pub status: CriterionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub criterion: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub question: String,
    pub decision: String,
    pub decided_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// The creator's copy of an assigned task (spec §5.3): a pointer, not a
/// record — the worker's session is the live one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPointer {
    pub session: String,
    pub status: String,
}

/// The user entry's payload (spec §5): the message text and its routing
/// (lane, the sender for a forwarded one, the invoked skill).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPayload {
    pub text: String,
    pub lane: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillRef>,
}

/// The invoked skill (the `/skill:` expansion, ticket #28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRef {
    pub name: String,
    pub location: String,
}

/// The assistant entry's payload (spec §6): the turn's output, its usage,
/// and the calls it made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantPayload {
    pub text: String,
    pub reasoning: String,
    pub interrupted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TurnUsage>,
    pub calls: Vec<FunctionCall>,
}

/// The tool entry's payload (spec §5.4): one call, its arguments, and the
/// result as the dispatcher returned it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPayload {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    pub output: ToolOutput,
}

/// A tool result's content (#34): plain text — the common, cheap path —
/// or an image block a vision-capable endpoint can see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolOutput {
    Text(String),
    Image(ImageBlock),
}

/// An image block (#34): the media type and the base64 payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub media_type: String,
    pub data_base64: String,
}
impl From<String> for ToolOutput {
    fn from(s: String) -> Self {
        ToolOutput::Text(s)
    }
}

impl From<&str> for ToolOutput {
    fn from(s: &str) -> Self {
        ToolOutput::Text(s.to_owned())
    }
}

/// The system entry's payload: a note the session records (a runaway
/// stop, a model change, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPayload {
    pub note: String,
}

/// The spawn-snapshot entry's payload (ticket #22): a compaction child
/// starts from the parent's frozen observation log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnSnapshotPayload {
    #[serde(rename = "parentSession")]
    pub parent_session: String,
    pub range: String,
    pub log: String,
}

/// The entry payloads share one serialization: the file line stays
/// free-form at the storage level (ADR-0005); the type is the owner of
/// its shape.
macro_rules! payload_value {
    ($t:ty) => {
        impl $t {
            pub fn to_value(&self) -> Value {
                serde_json::to_value(self).unwrap_or(Value::Null)
            }
        }
    };
}
payload_value!(UserPayload);
payload_value!(AssistantPayload);
payload_value!(ToolPayload);
payload_value!(SystemPayload);
payload_value!(SpawnSnapshotPayload);

/// Usage from the `response.completed` event (spec §6). Field names accept
/// both the Responses shape and the chat-completions dialect.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnUsage {
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

impl TurnUsage {
    #[must_use]
    pub fn reasoning_tokens(&self) -> u64 {
        self.output_tokens_details
            .as_ref()
            .map_or(0, |d| d.reasoning_tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #34: text stays a bare JSON string (the common, cheap path); an
    /// image is the tagged block.
    #[test]
    fn tool_output_payload_shape() {
        let text = ToolPayload {
            call_id: "c1".into(),
            name: "read".into(),
            args: json!({"path": "a.txt"}),
            output: ToolOutput::Text("line".into()),
        };
        assert_eq!(text.to_value()["output"], "line");

        let image = ToolPayload {
            call_id: "c2".into(),
            name: "read".into(),
            args: json!({"path": "a.png"}),
            output: ToolOutput::Image(ImageBlock {
                kind: "image".into(),
                media_type: "image/png".into(),
                data_base64: "AAAA".into(),
            }),
        };
        assert_eq!(
            image.to_value()["output"],
            json!({"type": "image", "media_type": "image/png", "data_base64": "AAAA"})
        );
    }

    #[test]
    fn step_and_criterion_status_strings() {
        assert_eq!(StepStatus::Pending.as_str(), "pending");
        assert_eq!(StepStatus::Active.as_str(), "active");
        assert_eq!(StepStatus::Done.as_str(), "done");
        assert_eq!(StepStatus::Skipped.as_str(), "skipped");
        assert_eq!(CriterionStatus::Pending.as_str(), "pending");
        assert_eq!(CriterionStatus::Satisfied.as_str(), "satisfied");
        assert_eq!(CriterionStatus::Failed.as_str(), "failed");
        assert_eq!(CriterionStatus::Skipped.as_str(), "skipped");
    }

    #[test]
    fn tool_output_from_conversions() {
        assert_eq!(ToolOutput::from("plain"), ToolOutput::Text("plain".into()));
        assert_eq!(
            ToolOutput::from(String::from("owned")),
            ToolOutput::Text("owned".into())
        );
    }

    /// The `skip_serializing_if` wire contract: unset optionals are absent from
    /// the entry line, not serialized as nulls (ADR-0005 free-form line).
    /// spec §6: usage arrives in the Responses shape or the chat-completions
    /// dialect; the aliases fold both onto one field.
    #[test]
    fn turn_usage_accepts_chat_completions_aliases() {
        let u: TurnUsage =
            serde_json::from_str(r#"{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}"#)
                .unwrap();
        assert_eq!(u.input_tokens, 11);
        assert_eq!(u.output_tokens, 7);
        assert_eq!(u.total_tokens, 18);

        // The Responses shape and a bare `{}` both decode (#[serde(default)]).
        let u: TurnUsage =
            serde_json::from_str(r#"{"input_tokens":3,"output_tokens":4,"total_tokens":7}"#)
                .unwrap();
        assert_eq!((u.input_tokens, u.output_tokens, u.total_tokens), (3, 4, 7));
        let u: TurnUsage = serde_json::from_str("{}").unwrap();
        assert_eq!(u, TurnUsage::default());
    }

    #[test]
    fn turn_usage_roundtrips_and_reasoning_tokens() {
        let u = TurnUsage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            output_tokens_details: Some(OutputTokensDetails {
                reasoning_tokens: 2,
            }),
            prompt_tokens_details: Some(PromptTokensDetails { cached_tokens: 1 }),
        };
        let back: TurnUsage = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!(back, u);
        assert_eq!(u.reasoning_tokens(), 2);
        assert_eq!(TurnUsage::default().reasoning_tokens(), 0);
    }
}

mod task;

pub use task::*;
