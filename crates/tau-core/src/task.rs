//! Per-session, event-sourced tasks (spec §5.3, ADR-0001/0006): a task is a
//! fold of append-only `task` entries in the owning session's file — state,
//! not an agent; it dies with the session. On assignment the record copies
//! into the worker's session, which becomes the live record; the creator's
//! copy becomes a status pointer.
use serde_json::Value;
pub use tau_protocol::payload::{
    Blocker, Criterion, CriterionStatus, Decision, Evidence, ResumeContract, Step, StepStatus,
    Task, TaskEvent, TaskRecord, WorkerPointer, resume_contract,
};

pub const KIND_TASK: &str = "task";

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_IN_PROGRESS: &str = "in_progress";
pub const STATUS_DONE: &str = "done";
pub const STATUS_BLOCKED: &str = "blocked";
pub const STATUS_CANCELLED: &str = "cancelled";

/// Fold a session's task entries into current task state. Task state is
/// session-scoped, not branch-scoped (spec §5.3): the fold walks every
/// entry, so forking does not destroy a task created on the parent branch.
pub fn fold_entries(entries: &[crate::session::Entry]) -> Vec<Task> {
    let mut tasks: std::collections::BTreeMap<String, Task> = std::collections::BTreeMap::new();
    for e in entries {
        if e.kind != KIND_TASK {
            continue;
        }
        let Some((id, event)) = TaskEvent::from_value(&e.payload) else {
            continue;
        };
        let task = tasks.entry(id.clone()).or_insert_with(|| Task {
            id: id.clone(),
            title: String::new(),
            status: STATUS_PENDING.to_owned(),
            steps: Vec::new(),
            criteria: Vec::new(),
            evidence: Vec::new(),
            blockers: Vec::new(),
            decisions: Vec::new(),
            notes: Vec::new(),
            worker: None,
            created_in: None,
            updated: 0,
        });
        apply_event(task, &event);
        task.updated = e.timestamp;
    }
    tasks.into_values().collect()
}

fn apply_event(task: &mut Task, event: &TaskEvent) {
    match event {
        TaskEvent::Created {
            title,
            steps,
            criteria,
        } => {
            if task.status == STATUS_PENDING {
                task.title = title.clone();
                task.steps = steps.clone();
                task.criteria = criteria.clone();
            }
        }
        // On the creator's session: the task is assigned away (pointer).
        // On the worker's session: the record copy arrives (full state).
        TaskEvent::Assigned { worker, record } => {
            if let Some(worker) = worker {
                task.status = STATUS_IN_PROGRESS.to_owned();
                task.worker = Some(WorkerPointer {
                    session: worker.clone(),
                    status: STATUS_IN_PROGRESS.to_owned(),
                });
            }
            if let Some(record) = record {
                task.title = record.title.clone();
                task.status = record.status.clone();
                task.steps = record.steps.clone();
                task.criteria = record.criteria.clone();
                task.evidence = record.evidence.clone();
                task.blockers = record.blockers.clone();
                task.created_in = Some(record.created_in.clone());
            }
        }
        TaskEvent::Started => {
            if task.status == STATUS_PENDING || task.status == STATUS_BLOCKED {
                task.status = STATUS_IN_PROGRESS.to_owned();
                advance_step(task);
            }
        }
        TaskEvent::Evidence { evidence } => {
            let ev = evidence.clone();
            if let Some(c) = task.criteria.iter_mut().find(|c| c.text == ev.criterion) {
                c.status = if ev.passed {
                    CriterionStatus::Satisfied
                } else {
                    CriterionStatus::Failed
                };
            }
            if let Some(step) = ev.step.as_deref()
                && let Some(s) = task.steps.iter_mut().find(|s| s.text == step)
            {
                s.status = StepStatus::Done;
                advance_step(task);
            }
            task.evidence.push(ev);
        }
        TaskEvent::Blocked { reason, needs } => {
            task.blockers.push(Blocker {
                reason: reason.clone(),
                needs: needs.clone(),
            });
            task.status = STATUS_BLOCKED.to_owned();
        }
        TaskEvent::Finished { force, reason } => {
            for c in task.criteria.iter_mut() {
                if c.status == CriterionStatus::Pending || c.status == CriterionStatus::Failed {
                    c.status = CriterionStatus::Skipped;
                }
            }
            if *force && let Some(reason) = reason {
                task.decisions.push(Decision {
                    question: format!("task {} finished by force", task.id),
                    decision: "force finish".to_owned(),
                    decided_by: "agent".to_owned(),
                    rationale: Some(reason.clone()),
                });
            }
            task.status = STATUS_DONE.to_owned();
        }
        TaskEvent::Cancelled { reason } => {
            if let Some(reason) = reason {
                task.notes.push(reason.clone());
            }
            task.status = STATUS_CANCELLED.to_owned();
        }
        TaskEvent::HandedOff { output } => {
            task.notes
                .push(serde_json::to_string(output).unwrap_or_default());
            task.decisions.push(Decision {
                question: format!("task {} resolution on child exit", task.id),
                decision: "handed_off".to_owned(),
                decided_by: "agent".to_owned(),
                rationale: None,
            });
        }
        // The creator's pointer tracks the worker's task status.
        TaskEvent::Pointer { status } => {
            if let Some(w) = task.worker.as_mut() {
                w.status = status.clone();
            }
        }
        TaskEvent::Note { text } => {
            task.notes.push(text.clone());
        }
        TaskEvent::Decision {
            question,
            decision,
            decided_by,
            rationale,
        } => {
            task.decisions.push(Decision {
                question: question.clone(),
                decision: decision.clone(),
                decided_by: decided_by.clone(),
                rationale: rationale.clone(),
            });
        }
    }
}

fn advance_step(task: &mut Task) {
    if task.steps.iter().any(|s| s.status == StepStatus::Active) {
        return;
    }
    if let Some(next) = task
        .steps
        .iter_mut()
        .find(|s| s.status == StepStatus::Pending)
    {
        next.status = StepStatus::Active;
    }
}

/// The active tasks of a session (the ones a context assembly carries the
/// resume contract for): in-progress, not the creator's pointer copy of an
/// assigned task (the creator tracks it, the worker works it).
pub fn active_tasks(tasks: &[Task]) -> Vec<&Task> {
    tasks
        .iter()
        .filter(|t| t.status == STATUS_IN_PROGRESS && t.worker.is_none())
        .collect()
}

use crate::session::SessionStore;

type StoreResult<T> = Result<T, String>;

fn load(store: &SessionStore) -> StoreResult<Vec<Task>> {
    store
        .entries_range(0, usize::MAX)
        .map(|v| fold_entries(&v))
        .map_err(|e| e.to_string())
}

fn find(store: &SessionStore, id: &str) -> StoreResult<Option<Task>> {
    Ok(load(store)?.into_iter().find(|t| t.id == id))
}

fn append_event(store: &mut SessionStore, id: &str, event: TaskEvent) -> StoreResult<()> {
    let payload = event.to_value(id);
    // Task events append to the active branch like all entries (spec
    // §5.3): a null parent would fork the conversation onto a phantom
    // root, since every later entry chains from the leaf.
    let parent = match store.leaf() {
        Ok(leaf) => leaf.map(|e| e.id),
        Err(e) => return Err(e.to_string()),
    };
    store
        .append(KIND_TASK, payload, parent.as_deref())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

mod store;
mod tool;
pub use store::*;
pub use tool::*;

#[cfg(test)]
mod tests;
