//! The task payload family (spec §5.3): the task's state types, the
//! event-sourced task log's entry, and the resume contract derived from
//! state.

use super::{
    Blocker, Criterion, CriterionStatus, Decision, Deserialize, Evidence, Serialize, Step,
    StepStatus, Value, WorkerPointer, json,
};

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
    #[must_use]
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
#[must_use]
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
        task.blockers.last().map_or_else(
            || "unblock".to_owned(),
            |b| {
                if let Some(needs) = &b.needs {
                    format!("unblock: {needs}")
                } else {
                    format!("unblock: {}", b.reason)
                }
            },
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_wire_omits_unset_optionals() {
        let bare = Evidence {
            criterion: "c".into(),
            summary: "s".into(),
            command: None,
            artifact: None,
            passed: true,
            step: None,
        };
        let v = serde_json::to_value(&bare).unwrap();
        assert!(v.get("command").is_none());
        assert!(v.get("artifact").is_none());
        assert!(v.get("step").is_none());

        let full = Evidence {
            command: Some("cargo test".into()),
            artifact: Some("out.txt".into()),
            step: Some("s1".into()),
            ..bare
        };
        let v = serde_json::to_value(&full).unwrap();
        assert_eq!(v["command"], "cargo test");
        assert_eq!(v["artifact"], "out.txt");
        assert_eq!(v["step"], "s1");
    }

    /// The task fold's contract: every variant survives a `to_value` →
    /// `from_value` round trip with its id intact.
    #[test]
    fn task_event_value_roundtrip() {
        let events = vec![
            TaskEvent::Created {
                title: "t".into(),
                steps: vec![Step {
                    text: "s".into(),
                    expected_output: "e".into(),
                    status: StepStatus::Pending,
                }],
                criteria: vec![Criterion {
                    text: "c".into(),
                    status: CriterionStatus::Pending,
                }],
            },
            TaskEvent::Assigned {
                worker: Some("h1".into()),
                record: Some(TaskRecord {
                    title: "t".into(),
                    status: "pending".into(),
                    steps: vec![],
                    criteria: vec![],
                    evidence: vec![],
                    blockers: vec![],
                    created_in: "s1".into(),
                }),
            },
            TaskEvent::Assigned {
                worker: None,
                record: None,
            },
            TaskEvent::Started,
            TaskEvent::Evidence {
                evidence: Evidence {
                    criterion: "c".into(),
                    summary: "s".into(),
                    command: None,
                    artifact: None,
                    passed: false,
                    step: None,
                },
            },
            TaskEvent::Blocked {
                reason: "r".into(),
                needs: Some("n".into()),
            },
            TaskEvent::Blocked {
                reason: "r".into(),
                needs: None,
            },
            TaskEvent::Finished {
                force: true,
                reason: Some("done early".into()),
            },
            TaskEvent::Finished {
                force: false,
                reason: None,
            },
            TaskEvent::Cancelled {
                reason: Some("r".into()),
            },
            TaskEvent::Cancelled { reason: None },
            TaskEvent::HandedOff {
                output: json!({ "result": "ok" }),
            },
            TaskEvent::Pointer {
                status: "done".into(),
            },
            TaskEvent::Note { text: "n".into() },
            TaskEvent::Decision {
                question: "q".into(),
                decision: "d".into(),
                decided_by: "a".into(),
                rationale: Some("r".into()),
            },
            TaskEvent::Decision {
                question: "q".into(),
                decision: "d".into(),
                decided_by: "a".into(),
                rationale: None,
            },
        ];
        for event in &events {
            let v = event.to_value("t-1");
            let (id, back) =
                TaskEvent::from_value(&v).unwrap_or_else(|| panic!("from_value lost {v:?}"));
            assert_eq!(id, "t-1");
            assert_eq!(&back, event);
        }
    }

    #[test]
    fn task_event_from_value_refuses_malformed() {
        // No id: the fold cannot attribute the event to a task.
        assert!(TaskEvent::from_value(&json!({ "event": "started" })).is_none());
        // Unknown tag: decodes to nothing, never an error to the caller.
        assert!(TaskEvent::from_value(&json!({ "id": "t", "event": "bogus" })).is_none());
        // Not an object at all.
        assert!(TaskEvent::from_value(&json!("started")).is_none());
        // Known tag, missing required fields.
        assert!(TaskEvent::from_value(&json!({ "id": "t", "event": "created" })).is_none());
    }

    fn task(status: &str) -> Task {
        Task {
            id: "t-1".into(),
            title: "do it".into(),
            status: status.into(),
            steps: vec![],
            criteria: vec![],
            evidence: vec![],
            blockers: vec![],
            decisions: vec![],
            notes: vec![],
            worker: None,
            created_in: None,
            updated: 0,
        }
    }

    /// spec §5.3: the resume contract's `next_action` is the compaction-safety
    /// payload — each status branch must produce its exact wording.
    #[test]
    fn resume_contract_next_action_per_state() {
        assert_eq!(resume_contract(&task("done")).next_action, "done");
        assert_eq!(resume_contract(&task("cancelled")).next_action, "cancelled");

        let mut t = task("blocked");
        t.blockers = vec![Blocker {
            reason: "waiting on api".into(),
            needs: Some("key".into()),
        }];
        assert_eq!(resume_contract(&t).next_action, "unblock: key");

        t.blockers = vec![Blocker {
            reason: "waiting on api".into(),
            needs: None,
        }];
        assert_eq!(resume_contract(&t).next_action, "unblock: waiting on api");

        t.blockers = vec![];
        assert_eq!(resume_contract(&t).next_action, "unblock");

        t.status = "active".into();
        t.steps = vec![
            Step {
                text: "write the parser".into(),
                expected_output: "tests pass".into(),
                status: StepStatus::Pending,
            },
            Step {
                text: "wire it up".into(),
                expected_output: "app builds".into(),
                status: StepStatus::Active,
            },
        ];
        let c = resume_contract(&t);
        assert_eq!(c.next_action, "complete: wire it up (expected: app builds)");
        assert_eq!(c.current_step.as_ref().unwrap().text, "wire it up");

        t.status = "active".into();
        t.steps = vec![];
        t.criteria = vec![
            Criterion {
                text: "fast".into(),
                status: CriterionStatus::Pending,
            },
            Criterion {
                text: "correct".into(),
                status: CriterionStatus::Satisfied,
            },
            Criterion {
                text: "documented".into(),
                status: CriterionStatus::Pending,
            },
        ];
        let c = resume_contract(&t);
        assert_eq!(
            c.next_action,
            "satisfy the outstanding criteria: fast; documented"
        );
        assert_eq!(c.gaps, vec!["fast".to_owned(), "documented".to_owned()]);

        t.criteria = vec![Criterion {
            text: "fast".into(),
            status: CriterionStatus::Satisfied,
        }];
        assert_eq!(resume_contract(&t).next_action, "finish the task");
        assert!(resume_contract(&t).gaps.is_empty());
    }

    #[test]
    fn resume_contract_copies_task_identity() {
        let mut t = task("active");
        t.evidence = vec![Evidence {
            criterion: "c".into(),
            summary: "s".into(),
            command: None,
            artifact: None,
            passed: true,
            step: None,
        }];
        let c = resume_contract(&t);
        assert_eq!(c.task, "t-1");
        assert_eq!(c.title, "do it");
        assert_eq!(c.status, "active");
        assert_eq!(c.evidence, t.evidence);
    }

    #[test]
    fn task_wire_omits_unset_optionals() {
        let v = serde_json::to_value(task("pending")).unwrap();
        assert!(v.get("worker").is_none());
        assert!(v.get("created_in").is_none());

        let mut t = task("pending");
        t.worker = Some(WorkerPointer {
            session: "s2".into(),
            status: "running".into(),
        });
        t.created_in = Some("s1".into());
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["worker"]["session"], "s2");
        assert_eq!(v["created_in"], "s1");
    }

    #[test]
    fn task_parse_negatives() {
        // Missing required fields are data errors; the fold and the paged
        // reads rely on Err, never a panic, for a corrupted line.
        let err = serde_json::from_str::<Task>(r#"{"title":"t"}"#).unwrap_err();
        assert_eq!(err.classify(), serde_json::error::Category::Data);
        // A truncated line (a torn write) is an Eof, not a panic.
        let err = serde_json::from_str::<Task>("{\"id\":").unwrap_err();
        assert_eq!(err.classify(), serde_json::error::Category::Eof);
    }
}
