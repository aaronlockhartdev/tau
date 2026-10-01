use super::{
    Criterion, CriterionStatus, Evidence, STATUS_BLOCKED, STATUS_CANCELLED, STATUS_DONE,
    STATUS_IN_PROGRESS, STATUS_PENDING, SessionStore, Step, StoreResult, Task, TaskEvent, Value,
    append_event, find,
};

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
        &TaskEvent::Created {
            title: title.to_owned(),
            steps,
            criteria,
        },
    )
}

/// Assignment (spec §5.3, ADR-0001): the record never leaves the
/// creator's session — the single source of truth. `worker_session` is
/// the worker's session id; the task stays in the creator's file with its
/// worker pointer set, and the worker's pane is a projection of it.
pub fn assign(creator: &mut SessionStore, id: &str, worker_session: &str) -> StoreResult<Task> {
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
    append_event(
        creator,
        id,
        &TaskEvent::Assigned {
            worker: Some(worker_session.to_owned()),
            record: None,
        },
    )?;
    find(creator, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
}

pub fn start(store: &mut SessionStore, id: &str) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status != STATUS_PENDING && task.status != STATUS_BLOCKED {
        return Err(format!("task {id}: cannot start from {}", task.status));
    }
    append_event(store, id, &TaskEvent::Started)?;
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
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
    append_event(store, id, &TaskEvent::Evidence { evidence })?;
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
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
        &TaskEvent::Blocked {
            reason: reason.to_owned(),
            needs,
        },
    )?;
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
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
    append_event(store, id, &TaskEvent::Finished { force, reason })?;
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
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
    append_event(
        store,
        id,
        &TaskEvent::HandedOff {
            output: output.clone(),
        },
    )?;
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
}

pub fn cancel(store: &mut SessionStore, id: &str, reason: Option<String>) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status == STATUS_DONE {
        return Err(format!("task {id}: done tasks do not cancel"));
    }
    append_event(store, id, &TaskEvent::Cancelled { reason })?;
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))
}

pub fn note(store: &mut SessionStore, id: &str, text: &str) -> StoreResult<()> {
    find(store, id)?.ok_or_else(|| format!("task {id}: not found in this session"))?;
    append_event(
        store,
        id,
        &TaskEvent::Note {
            text: text.to_owned(),
        },
    )
}
