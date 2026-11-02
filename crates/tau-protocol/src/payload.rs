//! The entry payload surface (C9): one owner for the shape each entry
//! kind carries across the core↔GUI seam. The session file stays
//! free-form at the ADR-0005 storage level; the types here are what the
//! writers construct and what the projections carry (the snapshot and
//! `LiveState` shapes are built from them, never from raw maps).

use crate::EntryKind;
use crate::diagnostic::StreamOutcome;
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
    /// How the provider's response ended, per its own account (#307), recorded
    /// when the call settles. Not `interrupted`: that is the user's intent for
    /// the turn and can be true on a stream the provider finished cleanly.
    /// `None` says no ending is named yet — a streaming snapshot, or a record
    /// written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StreamOutcome>,
    /// Read a stored one with [`TurnUsage::from_record`], which also covers the
    /// shape written before the record stopped mirroring one API.
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// #257: set when the image was downscaled to the pixel budget — the
    /// coordinate-mapping note the model must see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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

/// The typed entry payload (ticket #346, spec §8): the shape each entry
/// kind carries across the core↔GUI seam. The session file stays free-form
/// at the ADR-0005 storage level; `EntryPayload` is what the wire carries —
/// so a payload added in Rust can no longer be invisible to the parity
/// guard, which is the C9 hole this closes.
///
/// Adjacently tagged (`{type, data}`), not internally: serde rejects a
/// newtype variant on an internally tagged enum, and the payload types are
/// reused verbatim rather than re-declared field-for-field. `Other` keeps
/// forward compatibility (a kind this build has no type for rides its raw
/// JSON, the `EntryKind::Other` / `Diagnostic::Unknown` rule).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum EntryPayload {
    /// A user entry's payload (spec §5).
    User(UserPayload),
    /// An assistant entry's payload (spec §6).
    Assistant(AssistantPayload),
    /// A tool entry's payload (spec §5.4).
    Tool(ToolPayload),
    /// A system entry's payload.
    System(SystemPayload),
    /// A spawn-snapshot entry's payload (ticket #22).
    SpawnSnapshot(SpawnSnapshotPayload),
    /// A kind with no typed payload in v0: `om`, `subagent`, `diagnostic`,
    /// and any kind an older core has never heard of.
    Other(Value),
}

impl EntryPayload {
    /// Project a stored payload (the entry's JSONL line) onto the wire type.
    /// `None` is the sidecar case (ADR-0005): the payload lives in a blob and
    /// the entry carries only the pointer.
    ///
    /// A payload that does not decode as its kind's shape lands in `Other`
    /// with its bytes intact: a page read shows a record it cannot type, it
    /// never drops one. That is the storage-level freedom ADR-0005 allows —
    /// free-form at rest, typed in transit.
    #[must_use]
    pub fn from_entry(kind: &EntryKind, payload: &Value) -> Option<Self> {
        if payload.is_null() {
            return None;
        }
        let typed = match kind {
            EntryKind::User => {
                serde_json::from_value::<UserPayload>(payload.clone()).map(Self::User)
            }
            EntryKind::Assistant => {
                serde_json::from_value::<AssistantPayload>(payload.clone()).map(Self::Assistant)
            }
            EntryKind::Tool => {
                serde_json::from_value::<ToolPayload>(payload.clone()).map(Self::Tool)
            }
            EntryKind::System => {
                serde_json::from_value::<SystemPayload>(payload.clone()).map(Self::System)
            }
            EntryKind::SpawnSnapshot => {
                serde_json::from_value::<SpawnSnapshotPayload>(payload.clone())
                    .map(Self::SpawnSnapshot)
            }
            EntryKind::Om | EntryKind::Subagent | EntryKind::Diagnostic | EntryKind::Other(_) => {
                return Some(Self::Other(payload.clone()));
            }
        };
        Some(typed.unwrap_or_else(|_| Self::Other(payload.clone())))
    }

    /// The payload's JSON as a flat object, the shape the session file
    /// stores. A consumer that has not adopted the typed shape (the ACP
    /// mapper reads free-form fields) reaches the wire through here.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::User(p) => p.to_value(),
            Self::Assistant(p) => p.to_value(),
            Self::Tool(p) => p.to_value(),
            Self::System(p) => p.to_value(),
            Self::SpawnSnapshot(p) => p.to_value(),
            Self::Other(raw) => raw.clone(),
        }
    }
}

/// What a provider reported consuming for one response, as the record keeps
/// it. The stored shape names the concepts, never one API's spelling (§6.5):
/// each provider decodes its own wire shape into this (`provider::usage`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Served from the provider's prefix cache instead of recomputed. Always a
    /// *subset* of `input_tokens`, never an addition: an API that bills cache
    /// reads apart (Anthropic's `cache_read_input_tokens`) is folded in by its
    /// adapter at decode, so a consumer can add these fields without knowing
    /// which API answered. `None` is the provider not saying, which is not the
    /// same fact as a reported zero.
    ///
    /// Read a stored one with [`TurnUsage::from_record`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
}

impl TurnUsage {
    /// Derived, never stored: recording the sum beside the parts is the two
    /// writers ADR-0034 refuses for a prose note next to typed fields.
    /// Saturating — both addends arrive off the network.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Read a usage object as a record holds it, including the shape written
    /// before the record stopped mirroring the Responses API: there the cached
    /// count lived in a nested `prompt_tokens_details` and the total was
    /// stored. Frozen — nothing writes that spelling any more, and a
    /// pre-existing session must not read its cache as empty (the same reason
    /// the audit channel keeps `apply_legacy`, ADR-0034). Anything reading a
    /// stored usage through `AssistantPayload` instead skips this and loses the
    /// cache count.
    #[must_use]
    pub fn from_record(value: &Value) -> Option<Self> {
        let mut usage: Self = serde_json::from_value(value.clone()).ok()?;
        if usage.cached_input_tokens.is_none() {
            usage.cached_input_tokens = value
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64);
        }
        Some(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Positive: a stored payload of a typed kind projects onto its arm, and
    /// the wire shape is the adjacent `{type, data}` tag.
    #[test]
    fn a_typed_kind_projects_onto_its_arm() {
        let stored = json!({ "text": "hello", "lane": "steering" });
        let wire = EntryPayload::from_entry(&EntryKind::User, &stored).unwrap();
        assert!(matches!(&wire, EntryPayload::User(p) if p.text == "hello"));
        assert_eq!(
            serde_json::to_value(&wire).unwrap(),
            json!({ "type": "user", "data": { "text": "hello", "lane": "steering" } })
        );
    }

    /// Positive: the sidecar case is `None` — the entry carries the pointer,
    /// not the payload (ADR-0005).
    #[test]
    fn a_null_payload_is_the_sidecar_case() {
        assert_eq!(
            EntryPayload::from_entry(&EntryKind::Assistant, &Value::Null),
            None
        );
    }

    /// Negative: a payload that is not its kind's shape keeps its bytes in
    /// `Other` — a page read never drops a record it cannot type.
    #[test]
    fn a_malformed_payload_survives_as_other() {
        let stored = json!("not an object");
        let wire = EntryPayload::from_entry(&EntryKind::User, &stored).unwrap();
        assert_eq!(wire, EntryPayload::Other(json!("not an object")));
        assert_eq!(wire.to_value(), stored, "the bytes are intact");
        assert_eq!(
            serde_json::to_value(&wire).unwrap(),
            json!({ "type": "other", "data": "not an object" }),
            "the escape arm tags like every other arm"
        );
    }

    /// The kinds with no v0 payload type — and an unknown kind — ride `Other`.
    #[test]
    fn an_untyped_kind_rides_other() {
        for kind in [
            EntryKind::Om,
            EntryKind::Subagent,
            EntryKind::Diagnostic,
            EntryKind::Other("quantum".into()),
        ] {
            let stored = json!({ "note": "from a newer core" });
            let wire = EntryPayload::from_entry(&kind, &stored).unwrap();
            assert_eq!(wire, EntryPayload::Other(stored));
        }
    }

    /// `to_value` is the inverse: the typed payload is the JSON the file stores.
    #[test]
    fn a_typed_payload_returns_its_stored_shape() {
        let stored = json!({ "note": "model changed" });
        let wire = EntryPayload::from_entry(&EntryKind::System, &stored).unwrap();
        assert_eq!(wire.to_value(), stored);
    }

    /// A tool payload round-trips with its GUI-only channel intact (#243).
    #[test]
    fn a_tool_payload_keeps_its_gui_channel() {
        let stored = json!({
            "call_id": "c1",
            "name": "edit",
            "args": {"path": "a.txt"},
            "output": "edited a.txt",
            "ui_detail": "--- a\n+++ b",
            "status": "ok"
        });
        let wire = EntryPayload::from_entry(&EntryKind::Tool, &stored).unwrap();
        assert!(
            matches!(&wire, EntryPayload::Tool(p)
                if matches!(&p.ui_detail, Some(ToolOutput::Text(s)) if s == "--- a\n+++ b")),
            "ui_detail survives the typed hop"
        );
    }
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
                note: None,
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

    /// The record's shape is the concepts (§6.5): the parts, the cache as its
    /// own field, and no stored sum.
    #[test]
    fn turn_usage_records_concepts_not_a_wire_shape() {
        let kept = TurnUsage::from_record(&json!({"input_tokens": 3, "output_tokens": 4})).unwrap();
        assert_eq!(kept.cached_input_tokens, None, "silent about the cache");
        assert_eq!(kept.total_tokens(), 7, "the total is derived");
        assert_eq!(
            serde_json::to_value(&kept).unwrap(),
            json!({"input_tokens": 3, "output_tokens": 4}),
            "an unknown cache is absent from the line, not null"
        );
    }

    /// Records written while the type mirrored the Responses API stored the sum
    /// and nested the cached count. One key only: serde's aliases were
    /// deserialize-only, so `input_tokens_details` never reached a line and the
    /// frozen read must not claim it did.
    #[test]
    fn an_older_usage_record_keeps_its_cached_count() {
        let read = TurnUsage::from_record(&json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "total_tokens": 120,
            "prompt_tokens_details": { "cached_tokens": 40 },
            "output_tokens_details": { "reasoning_tokens": 5 },
        }))
        .unwrap();
        assert_eq!(read.cached_input_tokens, Some(40));
        assert_eq!(
            read.total_tokens(),
            120,
            "the derived sum matches the stored one"
        );
    }

    /// Both addends arrive off the network, so the sum neither wraps nor
    /// panics on a gateway's nonsense.
    #[test]
    fn a_bizarre_usage_total_saturates() {
        let huge = TurnUsage {
            input_tokens: u64::MAX,
            output_tokens: 1,
            ..Default::default()
        };
        assert_eq!(huge.total_tokens(), u64::MAX);
    }

    #[test]
    fn a_malformed_usage_reads_as_absent() {
        assert_eq!(TurnUsage::from_record(&json!(null)), None);
        assert_eq!(
            TurnUsage::from_record(&json!("tokens")),
            None,
            "a line that is not an object carries no usage"
        );
    }
}
