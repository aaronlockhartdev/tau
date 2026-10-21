//! The entry payload surface (C9): one owner for the shape each entry
//! kind carries across the core↔GUI seam. The session file stays
//! free-form at the ADR-0005 storage level; the types here are what the
//! writers construct and what the projections carry (the snapshot and
//! `LiveState` shapes are built from them, never from raw maps).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A function call the model emitted on a turn (spec §5.4).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub id: String,
    pub call_id: String,
    pub name: String,
    /// Raw JSON, as the server sent it.
    pub arguments: String,
}

/// The resume contract (spec §5.3, ADR-0001 revision): the plan's next
/// action — the handoff a resumed session continues from. The session's
/// plan file is the record; this points at where it left off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeContract {
    pub next_action: String,
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
    /// #224: the server's decode-window duration (seconds);
    /// `None` when the provider omitted the created/completed timestamps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    pub calls: Vec<FunctionCall>,
}

/// The tool entry's payload (spec §5.4): one call, its arguments, the result
/// as the dispatcher returned it, and the core's success/failure verdict — the
/// source of truth for the GUI's status indicator. Absent while the call is in
/// flight (the call-phase record carries no output yet).
///
/// `output` is the model-facing channel (it rides into the assembled context);
/// `ui_detail` is the GUI-only channel (spec §5.4, ADR-0012): present on the
/// wire entry for the card, structurally ignored by context assembly. `None`
/// for every tool that does not split its result (the common case) — a payload
/// written before the field existed deserializes to `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPayload {
    pub call_id: String,
    pub name: String,
    pub args: Value,
    pub output: ToolOutput,
    /// GUI-only result detail (#243): the diff of an `edit`, the written
    /// content of a `write`, the anchor-stripped body of a `read`. Never
    /// assembled into the model context. Absent when the tool does not split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_detail: Option<ToolOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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

/// A child's activity window (#203): the tail of its observation log (the
/// distilled memory, up to the OM cursor) plus the un-observed raw tail,
/// projected compact and tail-capped by the core. `degraded` marks the
/// cursor-miss fallback (the window is the whole branch tail-capped) —
/// a young child's empty observation log is NOT degraded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChildActivity {
    /// The newest N observation groups, newest last (the child's `recall`
    /// handles: the 16-hex group ids).
    pub observations: Vec<ActivityGroup>,
    /// The un-observed raw tail since the cursor, projected compact.
    pub live: Vec<ActivityEntry>,
    /// True when the OM cursor was absent from the branch: the live tail
    /// fell back to the (tail-capped) whole branch.
    pub degraded: bool,
}

/// One observation group from the child's log (the `recall` handle is the
/// group's 16-hex id; `kind` marks Reflector rewrites).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityGroup {
    pub id: String,
    pub range: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The group's bullets, truncated to the display limit.
    pub content: String,
}

/// One raw tail entry, projected to what the GUI renders: role, the tool
/// name and verdict for `tool` entries, and a truncated text/args slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityEntry {
    /// "user" | "assistant" | "tool" (the raw entry kinds).
    pub role: String,
    /// The tool call's name (tool entries only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The core's success/failure verdict (tool entries, once returned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The entry's text (user/assistant) or the tool args/output (tool),
    /// truncated to the display limit.
    pub text: String,
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
    use serde_json::json;

    /// #34: text stays a bare JSON string (the common, cheap path); an
    /// image is the tagged block.
    #[test]
    fn tool_output_payload_shape() {
        let text = ToolPayload {
            call_id: "c1".into(),
            name: "read".into(),
            args: json!({"path": "a.txt"}),
            output: ToolOutput::Text("line".into()),
            ui_detail: None,
            status: None,
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
            ui_detail: None,
            status: None,
        };
        assert_eq!(
            image.to_value()["output"],
            json!({"type": "image", "media_type": "image/png", "data_base64": "AAAA"})
        );
    }

    /// #243: `ui_detail` is wire-optional (a `skip_serializing_if` Option):
    /// absent when `None`, present when set, and a payload written before the
    /// field existed deserializes with `None`.
    #[test]
    fn ui_detail_wire_contract() {
        let p = ToolPayload {
            call_id: "c1".into(),
            name: "write".into(),
            args: json!({"path": "a.txt"}),
            output: ToolOutput::Text("wrote a.txt (7 bytes, 2 lines)".into()),
            ui_detail: Some(ToolOutput::Text("hello\nworld".into())),
            status: Some("ok".into()),
        };
        let v = p.to_value();
        assert_eq!(v["ui_detail"], "hello\nworld");

        let bare = ToolPayload {
            call_id: "c1".into(),
            name: "bash".into(),
            args: json!({"command": "ls"}),
            output: ToolOutput::Text("ok".into()),
            ui_detail: None,
            status: None,
        };
        assert!(
            bare.to_value().get("ui_detail").is_none(),
            "a None ui_detail must stay off the wire line"
        );

        // A session file line written before the field existed decodes to None.
        let old: ToolPayload = serde_json::from_str(
            r#"{"call_id":"c1","name":"edit","args":{},"output":"edited a.txt","status":"ok"}"#,
        )
        .unwrap();
        assert!(old.ui_detail.is_none());
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
