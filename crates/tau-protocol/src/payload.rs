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
    pub fn as_str(&self) -> &'static str {
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
    pub fn as_str(&self) -> &'static str {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: String,
    pub steps: Vec<Step>,
    pub criteria: Vec<Criterion>,
    pub evidence: Vec<Evidence>,
    pub blockers: Vec<Blocker>,
    pub decisions: Vec<Decision>,
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerPointer>,
    /// Set on the worker's copy: the session the task was created in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_in: Option<String>,
    pub updated: u64,
}

/// The assignment's record copy (spec §5.3): the worker's session
/// receives the task's state as a whole — a subset of `Task`, since the
/// worker's copy is the live one and the remaining fields default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub title: String,
    pub status: String,
    pub steps: Vec<Step>,
    pub criteria: Vec<Criterion>,
    pub evidence: Vec<Evidence>,
    pub blockers: Vec<Blocker>,
    pub created_in: String,
}

/// The resume contract (spec §5.3): the compaction-safety device.
/// Derived from the task's state; re-injected into context assembly while
/// the task is active, and the payload for resuming a paused/done child.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeContract {
    pub task: String,
    pub title: String,
    pub status: String,
    pub current_step: Option<Step>,
    pub steps: Vec<Step>,
    pub evidence: Vec<Evidence>,
    pub gaps: Vec<String>,
    pub blockers: Vec<Blocker>,
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

/// One `task` entry (spec §5.3): the event-sourced task log. The writer
/// side constructs a variant; the fold reads one back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TaskEvent {
    Created {
        title: String,
        steps: Vec<Step>,
        criteria: Vec<Criterion>,
    },
    Assigned {
        #[serde(skip_serializing_if = "Option::is_none")]
        worker: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        record: Option<TaskRecord>,
    },
    Started,
    Evidence {
        evidence: Evidence,
    },
    Blocked {
        reason: String,
        #[serde(default)]
        needs: Option<String>,
    },
    Finished {
        force: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Cancelled {
        #[serde(default)]
        reason: Option<String>,
    },
    HandedOff {
        output: Value,
    },
    Pointer {
        status: String,
    },
    Note {
        text: String,
    },
    Decision {
        question: String,
        decision: String,
        decided_by: String,
        #[serde(default)]
        rationale: Option<String>,
    },
}

impl TaskEvent {
    /// The entry payload: the variant's fields plus the task id.
    pub fn to_value(&self, id: &str) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(obj) = v.as_object_mut() {
            obj.insert("id".into(), json!(id));
        }
        v
    }

    /// One entry's payload (the fold's input): the task id and the
    /// decoded variant; a payload without a known event tag decodes to
    /// nothing.
    pub fn from_value(v: &Value) -> Option<(String, Self)> {
        let id = v.get("id").and_then(Value::as_str)?.to_owned();
        let event = serde_json::from_value::<TaskEvent>(v.clone()).ok()?;
        Some((id, event))
    }
}

/// The resume contract derived from a task's current state (spec §5.3).
pub fn resume_contract(task: &Task) -> ResumeContract {
    let current = task
        .steps
        .iter()
        .find(|s| s.status == StepStatus::Active)
        .cloned();
    let gaps: Vec<String> = task
        .criteria
        .iter()
        .filter(|c| c.status != CriterionStatus::Satisfied)
        .map(|c| c.text.clone())
        .collect();
    let next_action = if task.status == "done" {
        "done".to_owned()
    } else if task.status == "cancelled" {
        "cancelled".to_owned()
    } else if task.status == "blocked" {
        task.blockers
            .last()
            .map(|b| {
                if let Some(needs) = &b.needs {
                    format!("unblock: {needs}")
                } else {
                    format!("unblock: {}", b.reason)
                }
            })
            .unwrap_or_else(|| "unblock".to_owned())
    } else if let Some(step) = current.as_ref() {
        format!(
            "complete: {} (expected: {})",
            step.text, step.expected_output
        )
    } else if !gaps.is_empty() {
        format!("satisfy the outstanding criteria: {}", gaps.join("; "))
    } else {
        "finish the task".to_owned()
    };
    ResumeContract {
        task: task.id.clone(),
        title: task.title.clone(),
        status: task.status.clone(),
        current_step: current,
        steps: task.steps.clone(),
        evidence: task.evidence.clone(),
        gaps,
        blockers: task.blockers.clone(),
        next_action,
    }
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
    pub fn reasoning_tokens(&self) -> u64 {
        self.output_tokens_details
            .as_ref()
            .map(|d| d.reasoning_tokens)
            .unwrap_or(0)
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
}
