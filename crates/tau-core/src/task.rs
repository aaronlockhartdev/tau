//! Per-session, event-sourced tasks (spec §5.3, ADR-0001/0006): a task is a
//! fold of append-only `task` entries in the owning session's file — state,
//! not an agent; it dies with the session. On assignment the record copies
//! into the worker's session, which becomes the live record; the creator's
//! copy becomes a status pointer.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const KIND_TASK: &str = "task";

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_IN_PROGRESS: &str = "in_progress";
pub const STATUS_DONE: &str = "done";
pub const STATUS_BLOCKED: &str = "blocked";
pub const STATUS_CANCELLED: &str = "cancelled";

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

/// Fold a session's task entries into current task state. Task state is
/// session-scoped, not branch-scoped (spec §5.3): the fold walks every
/// entry, so forking does not destroy a task created on the parent branch.
pub fn fold_entries(entries: &[crate::session::Entry]) -> Vec<Task> {
    let mut tasks: std::collections::BTreeMap<String, Task> = std::collections::BTreeMap::new();
    for e in entries {
        if e.kind != KIND_TASK {
            continue;
        }
        let Some(event) = e.payload.get("event").and_then(Value::as_str) else {
            continue;
        };
        let Some(id) = e.payload.get("id").and_then(Value::as_str) else {
            continue;
        };
        let task = tasks.entry(id.to_owned()).or_insert_with(|| Task {
            id: id.to_owned(),
            title: e
                .payload
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default(),
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
        apply_event(task, event, &e.payload);
        task.updated = e.timestamp;
    }
    tasks.into_values().collect()
}

fn apply_event(task: &mut Task, event: &str, p: &Value) {
    match event {
        "created" => {
            if task.status == STATUS_PENDING {
                if let Some(title) = p.get("title").and_then(Value::as_str) {
                    task.title = title.to_owned();
                }
                if let Some(steps) = p.get("steps") {
                    task.steps = serde_json::from_value(steps.clone()).unwrap_or_default();
                }
                if let Some(criteria) = p.get("criteria") {
                    task.criteria = serde_json::from_value(criteria.clone()).unwrap_or_default();
                }
            }
        }
        // On the creator's session: the task is assigned away (pointer).
        // On the worker's session: the record copy arrives (full state).
        "assigned" => {
            if let Some(worker) = p.get("worker").and_then(Value::as_str) {
                task.status = STATUS_IN_PROGRESS.to_owned();
                task.worker = Some(WorkerPointer {
                    session: worker.to_owned(),
                    status: STATUS_IN_PROGRESS.to_owned(),
                });
            }
            if p.get("record").is_some() {
                let record = p.get("record").unwrap();
                if let Some(title) = record.get("title").and_then(Value::as_str) {
                    task.title = title.to_owned();
                }
                if let Some(steps) = record.get("steps") {
                    task.steps = serde_json::from_value(steps.clone()).unwrap_or_default();
                }
                if let Some(criteria) = record.get("criteria") {
                    task.criteria = serde_json::from_value(criteria.clone()).unwrap_or_default();
                }
                if let Some(evidence) = record.get("evidence") {
                    task.evidence = serde_json::from_value(evidence.clone()).unwrap_or_default();
                }
                if let Some(blockers) = record.get("blockers") {
                    task.blockers = serde_json::from_value(blockers.clone()).unwrap_or_default();
                }
                task.status = record
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or(STATUS_IN_PROGRESS)
                    .to_owned();
                if let Some(created_in) = record.get("created_in").and_then(Value::as_str) {
                    task.created_in = Some(created_in.to_owned());
                }
            }
        }
        "started" => {
            if task.status == STATUS_PENDING || task.status == STATUS_BLOCKED {
                task.status = STATUS_IN_PROGRESS.to_owned();
                advance_step(task);
            }
        }
        "evidence" => {
            if let Some(ev) = p
                .get("evidence")
                .cloned()
                .and_then(|v| serde_json::from_value::<Evidence>(v).ok())
            {
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
        }
        "blocked" => {
            task.blockers.push(Blocker {
                reason: p
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                needs: p.get("needs").and_then(Value::as_str).map(str::to_owned),
            });
            task.status = STATUS_BLOCKED.to_owned();
        }
        "finished" => {
            let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
            for c in task.criteria.iter_mut() {
                if c.status == CriterionStatus::Pending || c.status == CriterionStatus::Failed {
                    c.status = CriterionStatus::Skipped;
                }
            }
            if force && let Some(reason) = p.get("reason").and_then(Value::as_str) {
                task.decisions.push(Decision {
                    question: format!("task {} finished by force", task.id),
                    decision: "force finish".to_owned(),
                    decided_by: "agent".to_owned(),
                    rationale: Some(reason.to_owned()),
                });
            }
            task.status = STATUS_DONE.to_owned();
        }
        "cancelled" => {
            if let Some(reason) = p.get("reason").and_then(Value::as_str) {
                task.notes.push(reason.to_owned());
            }
            task.status = STATUS_CANCELLED.to_owned();
        }
        "handed_off" => {
            if let Some(output) = p.get("output") {
                task.notes.push(output.to_string());
            }
            task.decisions.push(Decision {
                question: format!("task {} resolution on child exit", task.id),
                decision: "handed_off".to_owned(),
                decided_by: "agent".to_owned(),
                rationale: None,
            });
        }
        // The creator's pointer tracks the worker's task status.
        "pointer" => {
            if let Some(status) = p.get("status").and_then(Value::as_str)
                && let Some(w) = task.worker.as_mut()
            {
                w.status = status.to_owned();
            }
        }
        "note" => {
            if let Some(text) = p.get("text").and_then(Value::as_str) {
                task.notes.push(text.to_owned());
            }
        }
        "decision" => {
            task.decisions.push(Decision {
                question: p
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                decision: p
                    .get("decision")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                decided_by: p
                    .get("decided_by")
                    .and_then(Value::as_str)
                    .unwrap_or("agent")
                    .to_owned(),
                rationale: p
                    .get("rationale")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
        _ => {}
    }
}

/// A step's `done` activates the next pending one (the ordered plan, spec
/// §5.3); called from `started` and from evidence naming a finished step.
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

/// The resume contract (spec §5.3): the compaction-safety device. Derived
/// from the task's events; re-injected into context assembly while the
/// task is active, and the payload for resuming a paused/done child.
pub fn resume_contract(task: &Task) -> Value {
    let current = task
        .steps
        .iter()
        .find(|s| s.status == StepStatus::Active)
        .map(|s| json!({"text": s.text, "expected_output": s.expected_output}));
    let gaps: Vec<String> = task
        .criteria
        .iter()
        .filter(|c| c.status != CriterionStatus::Satisfied)
        .map(|c| c.text.clone())
        .collect();
    let next_action = if task.status == STATUS_DONE {
        "done".to_owned()
    } else if task.status == STATUS_CANCELLED {
        "cancelled".to_owned()
    } else if task.status == STATUS_BLOCKED {
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
            step["text"].as_str().unwrap_or_default(),
            step["expected_output"].as_str().unwrap_or_default()
        )
    } else if !gaps.is_empty() {
        format!("satisfy the outstanding criteria: {}", gaps.join("; "))
    } else {
        "finish the task".to_owned()
    };
    json!({
        "task": task.id,
        "title": task.title,
        "status": task.status,
        "current_step": current,
        "steps": task.steps,
        "evidence": task.evidence,
        "gaps": gaps,
        "blockers": task.blockers,
        "next_action": next_action,
    })
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

// --- Session-store operations -------------------------------------------

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

fn append_event(store: &mut SessionStore, id: &str, event: &str, extra: Value) -> StoreResult<()> {
    let mut payload = json!({ "event": event, "id": id });
    if let Some(obj) = payload.as_object_mut()
        && let Some(extra) = extra.as_object()
    {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
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

pub fn create(
    store: &mut SessionStore,
    id: &str,
    title: &str,
    steps: Vec<Step>,
    criteria: Vec<Criterion>,
) -> StoreResult<()> {
    append_event(
        store,
        id,
        "created",
        json!({
            "title": title,
            "steps": steps,
            "criteria": criteria,
        }),
    )
}

/// Assignment (spec §5.3, ADR-0001): the record copies into the worker's
/// session — which becomes the live record — and the creator's copy becomes
/// a status pointer. `worker_session` is the worker's session id; the
/// caller supplies the worker's open store (a spawn passes the child's).
pub fn assign(
    creator: &mut SessionStore,
    worker: &mut SessionStore,
    id: &str,
    worker_session: &str,
    creator_session: &str,
) -> StoreResult<Task> {
    let Some(task) = find(creator, id)?.filter(|t| t.worker.is_none()) else {
        return Err(format!(
            "task {id}: not found in this session (or already assigned)"
        ));
    };
    // A terminal task is closed, not pausable: assigning it would
    // resurrect it (review N7).
    if task.status == STATUS_DONE || task.status == STATUS_CANCELLED {
        return Err(format!("task {id}: cannot assign a {} task", task.status));
    }
    append_event(creator, id, "assigned", json!({ "worker": worker_session }))?;
    append_event(
        worker,
        id,
        "assigned",
        json!({
            "record": {
                "title": task.title,
                "status": STATUS_IN_PROGRESS,
                "steps": task.steps,
                "criteria": task.criteria,
                "evidence": task.evidence,
                "blockers": task.blockers,
                "created_in": creator_session,
            },
        }),
    )?;
    find(worker, id).map(|t| t.unwrap())
}

pub fn start(store: &mut SessionStore, id: &str) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status != STATUS_PENDING && task.status != STATUS_BLOCKED {
        return Err(format!("task {id}: cannot start from {}", task.status));
    }
    append_event(store, id, "started", json!({}))?;
    find(store, id).map(|t| t.unwrap())
}

pub fn add_evidence(store: &mut SessionStore, id: &str, evidence: Evidence) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status != STATUS_IN_PROGRESS {
        return Err(format!(
            "task {id}: evidence only while in_progress (it is {})",
            task.status
        ));
    }
    if !task.criteria.iter().any(|c| c.text == evidence.criterion) {
        return Err(format!(
            "task {id}: unknown criterion {:?}",
            evidence.criterion
        ));
    }
    append_event(store, id, "evidence", json!({ "evidence": evidence }))?;
    find(store, id).map(|t| t.unwrap())
}

pub fn block(
    store: &mut SessionStore,
    id: &str,
    reason: &str,
    needs: Option<String>,
) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status != STATUS_IN_PROGRESS {
        return Err(format!(
            "task {id}: only in_progress tasks block (it is {})",
            task.status
        ));
    }
    append_event(
        store,
        id,
        "blocked",
        json!({ "reason": reason, "needs": needs }),
    )?;
    find(store, id).map(|t| t.unwrap())
}

/// The done-resolution gate (spec §5.3): `done` only if every criterion is
/// satisfied by passing evidence; `force` + reason is the documented escape
/// (recorded as a decision event).
pub fn finish(
    store: &mut SessionStore,
    id: &str,
    force: bool,
    reason: Option<String>,
) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status != STATUS_IN_PROGRESS {
        return Err(format!(
            "task {id}: only in_progress tasks finish (it is {})",
            task.status
        ));
    }
    let gate = task
        .criteria
        .iter()
        .all(|c| c.status == CriterionStatus::Satisfied);
    if !gate && !force {
        let gaps: Vec<&str> = task
            .criteria
            .iter()
            .filter(|c| c.status != CriterionStatus::Satisfied)
            .map(|c| c.text.as_str())
            .collect();
        return Err(format!(
            "task {id}: completion gate failed — unsatisfied criteria: {}",
            gaps.join("; ")
        ));
    }
    // Force without a reason is how silent corruption would in: the
    // reason lands in the session entry, so the gate's bypass is auditable
    // even when the evidence is incomplete (review N8).
    if force && reason.is_none() {
        return Err(format!("task {id}: force finish requires a reason"));
    }
    append_event(
        store,
        id,
        "finished",
        json!({ "force": force, "reason": reason }),
    )?;
    find(store, id).map(|t| t.unwrap())
}

/// A child that cannot finish hands the task off (spec §5.3): it stays
/// `in_progress`, the output becomes the resume contract, the parent decides.
pub fn handoff(store: &mut SessionStore, id: &str, output: &Value) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status != STATUS_IN_PROGRESS {
        return Err(format!(
            "task {id}: only in_progress tasks hand off (it is {})",
            task.status
        ));
    }
    append_event(store, id, "handed_off", json!({ "output": output }))?;
    find(store, id).map(|t| t.unwrap())
}

pub fn cancel(store: &mut SessionStore, id: &str, reason: Option<String>) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status == STATUS_DONE {
        return Err(format!("task {id}: done tasks do not cancel"));
    }
    append_event(store, id, "cancelled", json!({ "reason": reason }))?;
    find(store, id).map(|t| t.unwrap())
}

/// The creator's pointer tracks the worker's task (the child's notify
/// events call this on the creator's copy).
pub fn mirror_status(creator: &mut SessionStore, id: &str, status: &str) -> StoreResult<()> {
    if find(creator, id)?.is_some() {
        append_event(creator, id, "pointer", json!({ "status": status }))
    } else {
        Ok(())
    }
}

pub fn note(store: &mut SessionStore, id: &str, text: &str) -> StoreResult<()> {
    append_event(store, id, "note", json!({ "text": text }))
}

// --- Model-facing tool routing ------------------------------------------

/// The seven task tools (spec §5.4): free text + ids in, core-enforced
/// transitions; diagnostics are results, never panics.
pub fn tool_call(store: &mut SessionStore, name: &str, args: &Value) -> String {
    match name {
        "task_create" => {
            let Some(title) = args.get("title").and_then(Value::as_str) else {
                return "task_create: missing \"title\"".into();
            };
            // Malformed items are rejected, not silently dropped (review
            // N6): a step without its expected output is a quality-gate
            // failure the model must see.
            let steps = match args.get("steps").and_then(|v| v.as_array()) {
                Some(v) => {
                    let mut out = Vec::new();
                    for (i, s) in v.iter().enumerate() {
                        let Some(text) = s.get("text").and_then(Value::as_str) else {
                            return format!("task_create: step {i} is missing \"text\"");
                        };
                        let Some(expected) = s.get("expected_output").and_then(Value::as_str)
                        else {
                            return format!(
                                "task_create: step {i} (\"{text}\") is missing \"expected_output\""
                            );
                        };
                        out.push(Step {
                            text: text.to_owned(),
                            expected_output: expected.to_owned(),
                            status: StepStatus::Pending,
                        });
                    }
                    out
                }
                None => Vec::new(),
            };
            let criteria = match args.get("criteria").and_then(|v| v.as_array()) {
                Some(v) => {
                    let mut out = Vec::new();
                    for (i, c) in v.iter().enumerate() {
                        let Some(text) = c.as_str() else {
                            return format!("task_create: criterion {i} is not a string");
                        };
                        out.push(Criterion {
                            text: text.to_owned(),
                            status: CriterionStatus::Pending,
                        });
                    }
                    out
                }
                None => Vec::new(),
            };
            let n = load(store)
                .map(|t| t.iter().filter(|t| t.id.starts_with("task-")).count())
                .unwrap_or(0)
                + 1;
            let id = format!("task-{n}");
            match create(store, &id, title, steps.clone(), criteria.clone()) {
                Ok(()) => format!(
                    "created {id}: {title} ({} steps, {} criteria)",
                    steps.len(),
                    criteria.len()
                ),
                Err(e) => e,
            }
        }
        "task_start" => {
            let Some(id) = task_id(args) else {
                return "task_start: missing \"task\"".into();
            };
            match start(store, id) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_evidence" => {
            let Some(id) = task_id(args) else {
                return "task_evidence: missing \"task\"".into();
            };
            let Some(criterion) = args.get("criterion").and_then(Value::as_str) else {
                return "task_evidence: missing \"criterion\"".into();
            };
            let Some(summary) = args.get("summary").and_then(Value::as_str) else {
                return "task_evidence: missing \"summary\"".into();
            };
            let ev = Evidence {
                criterion: criterion.to_owned(),
                summary: summary.to_owned(),
                command: args
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                artifact: args
                    .get("artifact")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                passed: args.get("passed").and_then(Value::as_bool).unwrap_or(true),
                step: args.get("step").and_then(Value::as_str).map(str::to_owned),
            };
            match add_evidence(store, id, ev) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_block" => {
            let Some(id) = task_id(args) else {
                return "task_block: missing \"task\"".into();
            };
            let Some(reason) = args.get("reason").and_then(Value::as_str) else {
                return "task_block: missing \"reason\"".into();
            };
            match block(
                store,
                id,
                reason,
                args.get("needs").and_then(Value::as_str).map(str::to_owned),
            ) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_finish" => {
            let Some(id) = task_id(args) else {
                return "task_finish: missing \"task\"".into();
            };
            match finish(
                store,
                id,
                args.get("force").and_then(Value::as_bool).unwrap_or(false),
                args.get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_cancel" => {
            let Some(id) = task_id(args) else {
                return "task_cancel: missing \"task\"".into();
            };
            match cancel(
                store,
                id,
                args.get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        other => format!("unknown task tool {other}"),
    }
}

fn task_id(args: &Value) -> Option<&str> {
    args.get("task").and_then(Value::as_str)
}

fn report(t: &Task) -> String {
    let worker = t
        .worker
        .as_ref()
        .map(|w| format!(" → worker {} ({})", w.session, w.status));
    format!(
        "task {} {:?} [{}]{}",
        t.id,
        t.title,
        t.status,
        worker.as_deref().unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionStore;

    fn session_in(dir: &std::path::Path, id: &str) -> SessionStore {
        let mut store = SessionStore::for_workspace(dir, id);
        store.create().unwrap();
        store
    }

    fn step(text: &str, expected: &str) -> Step {
        Step {
            text: text.into(),
            expected_output: expected.into(),
            status: StepStatus::Pending,
        }
    }

    fn criterion(text: &str) -> Criterion {
        Criterion {
            text: text.into(),
            status: CriterionStatus::Pending,
        }
    }

    #[test]
    fn fold_reconstructs_the_created_task() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        create(
            &mut store,
            "task-1",
            "write the docs",
            vec![step("draft", "docs/draft.md"), step("review", "merged PR")],
            vec![criterion("docs exist"), criterion("CI green")],
        )
        .unwrap();
        let tasks = load(&store).unwrap();
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.id, "task-1");
        assert_eq!(t.title, "write the docs");
        assert_eq!(t.status, STATUS_PENDING);
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.criteria.len(), 2);
    }

    #[test]
    fn state_machine_rejects_illegal_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        create(
            &mut store,
            "task-1",
            "t",
            vec![step("a", "b")],
            vec![criterion("c")],
        )
        .unwrap();
        // evidence before start is illegal
        let ev = Evidence {
            criterion: "c".into(),
            summary: "s".into(),
            command: None,
            artifact: None,
            passed: true,
            step: None,
        };
        assert!(add_evidence(&mut store, "task-1", ev).is_err());
        // start activates the first step
        let t = start(&mut store, "task-1").unwrap();
        assert_eq!(t.status, STATUS_IN_PROGRESS);
        assert_eq!(t.steps[0].status, StepStatus::Active);
        // block, then re-start from blocked
        block(&mut store, "task-1", "stuck", Some("needs ci".into())).unwrap();
        assert!(start(&mut store, "task-1").unwrap().status == STATUS_IN_PROGRESS);
        // double start is illegal
        assert!(start(&mut store, "task-1").is_err());
        // a done task rejects everything
        evidence_pass(&mut store);
        finish(&mut store, "task-1", false, None).unwrap();
        assert!(block(&mut store, "task-1", "x", None).is_err());
        assert!(cancel(&mut store, "task-1", None).is_err());
    }

    fn evidence_pass(store: &mut SessionStore) {
        add_evidence(
            store,
            "task-1",
            Evidence {
                criterion: "c".into(),
                summary: "did it".into(),
                command: Some("cargo test".into()),
                artifact: None,
                passed: true,
                step: Some("a".into()),
            },
        )
        .unwrap();
    }

    #[test]
    fn gate_requires_all_criteria_and_force_needs_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        create(
            &mut store,
            "task-1",
            "t",
            vec![],
            vec![criterion("one"), criterion("two")],
        )
        .unwrap();
        start(&mut store, "task-1").unwrap();
        add_evidence(
            &mut store,
            "task-1",
            Evidence {
                criterion: "one".into(),
                summary: "s".into(),
                command: None,
                artifact: None,
                passed: true,
                step: None,
            },
        )
        .unwrap();
        // one criterion left → gate fails
        let e = finish(&mut store, "task-1", false, None).unwrap_err();
        assert!(e.contains("two"), "{e}");
        // force without reason is rejected
        assert!(finish(&mut store, "task-1", true, None).is_err());
        // force with reason records a decision and skips the rest
        let t = finish(&mut store, "task-1", true, Some("user said ship it".into())).unwrap();
        assert_eq!(t.status, STATUS_DONE);
        assert_eq!(t.criteria[1].status, CriterionStatus::Skipped);
        assert_eq!(t.decisions.len(), 1);
        assert_eq!(
            t.decisions[0].rationale.as_deref(),
            Some("user said ship it")
        );
    }

    #[test]
    fn failing_evidence_marks_the_criterion_failed() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        create(&mut store, "task-1", "t", vec![], vec![criterion("c")]).unwrap();
        start(&mut store, "task-1").unwrap();
        add_evidence(
            &mut store,
            "task-1",
            Evidence {
                criterion: "c".into(),
                summary: "broke".into(),
                command: None,
                artifact: None,
                passed: false,
                step: None,
            },
        )
        .unwrap();
        let t = load(&store).unwrap()[0].clone();
        assert_eq!(t.criteria[0].status, CriterionStatus::Failed);
        // a later passing evidence re-satisfies it
        add_evidence(
            &mut store,
            "task-1",
            Evidence {
                criterion: "c".into(),
                summary: "fixed".into(),
                command: None,
                artifact: None,
                passed: true,
                step: None,
            },
        )
        .unwrap();
        let t = load(&store).unwrap()[0].clone();
        assert_eq!(t.criteria[0].status, CriterionStatus::Satisfied);
        assert_eq!(t.evidence.len(), 2);
    }

    #[test]
    fn assignment_copies_the_record_and_pointer_tracks_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut creator = session_in(dir.path(), "parent");
        let mut worker = session_in(dir.path(), "child");
        create(
            &mut creator,
            "task-1",
            "the brief",
            vec![step("do it", "done.md")],
            vec![criterion("proof")],
        )
        .unwrap();
        let live = assign(&mut creator, &mut worker, "task-1", "child", "parent").unwrap();
        assert_eq!(live.status, STATUS_IN_PROGRESS);
        assert_eq!(live.created_in.as_deref(), Some("parent"));
        // the creator's copy is now a pointer
        let c = load(&creator).unwrap()[0].clone();
        assert_eq!(c.status, STATUS_IN_PROGRESS);
        assert_eq!(c.worker.as_ref().unwrap().session, "child");
        // the worker's copy carries the full record (steps + criteria)
        let w = load(&worker).unwrap()[0].clone();
        assert_eq!(w.steps.len(), 1);
        assert_eq!(w.criteria.len(), 1);
        assert!(w.worker.is_none());
        // double assignment is rejected
        assert!(assign(&mut creator, &mut worker, "task-1", "child", "parent").is_err());
        // the pointer mirrors the worker's terminal status
        add_evidence(
            &mut worker,
            "task-1",
            Evidence {
                criterion: "proof".into(),
                summary: "s".into(),
                command: None,
                artifact: None,
                passed: true,
                step: None,
            },
        )
        .unwrap();
        finish(&mut worker, "task-1", false, None).unwrap();
        mirror_status(&mut creator, "task-1", STATUS_DONE).unwrap();
        let c = load(&creator).unwrap()[0].clone();
        assert_eq!(c.worker.as_ref().unwrap().status, STATUS_DONE);
    }

    #[test]
    fn resume_contract_carries_the_active_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        create(
            &mut store,
            "task-1",
            "t",
            vec![step("first", "a.md"), step("second", "b.md")],
            vec![criterion("done properly")],
        )
        .unwrap();
        start(&mut store, "task-1").unwrap();
        let c = resume_contract(&load(&store).unwrap()[0].clone());
        assert_eq!(c["task"], "task-1");
        assert_eq!(c["status"], STATUS_IN_PROGRESS);
        assert_eq!(c["current_step"]["text"], "first");
        assert_eq!(
            c["next_action"].as_str().unwrap(),
            "complete: first (expected: a.md)"
        );
        assert_eq!(c["gaps"].as_array().unwrap().len(), 1);
        // finishing a step advances the plan and the contract follows it
        add_evidence(
            &mut store,
            "task-1",
            Evidence {
                criterion: "done properly".into(),
                summary: "s".into(),
                command: None,
                artifact: None,
                passed: true,
                step: Some("first".into()),
            },
        )
        .unwrap();
        let t = load(&store).unwrap()[0].clone();
        assert_eq!(t.steps[0].status, StepStatus::Done);
        assert_eq!(t.steps[1].status, StepStatus::Active);
        let c = resume_contract(&t);
        assert_eq!(c["current_step"]["text"], "second");
        assert!(c["gaps"].as_array().unwrap().is_empty());
    }

    #[test]
    fn handoff_keeps_the_task_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        create(&mut store, "task-1", "t", vec![], vec![criterion("c")]).unwrap();
        start(&mut store, "task-1").unwrap();
        let t = handoff(&mut store, "task-1", &json!({"text": "waiting on ci"})).unwrap();
        assert_eq!(t.status, STATUS_IN_PROGRESS);
        assert!(t.notes.iter().any(|n| n.contains("waiting on ci")));
        assert_eq!(t.decisions[0].decision, "handed_off");
    }

    #[test]
    fn active_tasks_excludes_pointer_copies() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = session_in(dir.path(), "a");
        let mut b = session_in(dir.path(), "b");
        create(&mut a, "task-1", "t", vec![], vec![criterion("c")]).unwrap();
        start(&mut a, "task-1").unwrap();
        assert_eq!(active_tasks(&load(&a).unwrap()).len(), 1);
        assign(&mut a, &mut b, "task-1", "b", "a").unwrap();
        // a's copy is a pointer (not active for assembly); b's is the live one
        assert_eq!(active_tasks(&load(&a).unwrap()).len(), 0);
        assert_eq!(active_tasks(&load(&b).unwrap()).len(), 1);
    }

    #[test]
    fn task_events_sit_on_the_active_branch() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = session_in(dir.path(), "s1");
        store
            .append("user", json!({ "text": "start" }), None)
            .unwrap();
        create(
            &mut store,
            "task-1",
            "t",
            vec![step("a", "b")],
            vec![criterion("c")],
        )
        .unwrap();
        let task = store.leaf().unwrap().expect("the task event is the leaf");
        assert_eq!(task.kind, KIND_TASK);
        // The next conversation entry is written the way the loop writes it:
        // parent resolved from the current leaf.
        let leaf_id = store.leaf().unwrap().map(|e| e.id);
        let parent = leaf_id.as_deref();
        let later = store
            .append("user", json!({ "text": "next" }), parent)
            .unwrap();
        // The task is ON the branch: the next conversation entry chains from
        // it, and the leaf stays on the conversation (not the task root).
        assert_eq!(later.parent.as_deref(), Some(task.id.as_str()));
        assert_eq!(store.leaf().unwrap().unwrap().id, later.id);
    }
}
