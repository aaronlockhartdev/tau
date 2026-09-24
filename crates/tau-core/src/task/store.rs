use super::*;

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
        TaskEvent::Created {
            title: title.to_owned(),
            steps,
            criteria,
        },
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
    append_event(
        creator,
        id,
        TaskEvent::Assigned {
            worker: Some(worker_session.to_owned()),
            record: None,
        },
    )?;
    append_event(
        worker,
        id,
        TaskEvent::Assigned {
            worker: None,
            record: Some(TaskRecord {
                title: task.title.clone(),
                status: STATUS_IN_PROGRESS.to_owned(),
                steps: task.steps.clone(),
                criteria: task.criteria.clone(),
                evidence: task.evidence.clone(),
                blockers: task.blockers.clone(),
                created_in: creator_session.to_owned(),
            }),
        },
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
    append_event(store, id, TaskEvent::Started)?;
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
    append_event(store, id, TaskEvent::Evidence { evidence })?;
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
        TaskEvent::Blocked {
            reason: reason.to_owned(),
            needs,
        },
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
    append_event(store, id, TaskEvent::Finished { force, reason })?;
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
    append_event(
        store,
        id,
        TaskEvent::HandedOff {
            output: output.clone(),
        },
    )?;
    find(store, id).map(|t| t.unwrap())
}

pub fn cancel(store: &mut SessionStore, id: &str, reason: Option<String>) -> StoreResult<Task> {
    let Some(task) = find(store, id)? else {
        return Err(format!("task {id}: not found in this session"));
    };
    if task.status == STATUS_DONE {
        return Err(format!("task {id}: done tasks do not cancel"));
    }
    append_event(store, id, TaskEvent::Cancelled { reason })?;
    find(store, id).map(|t| t.unwrap())
}

/// The creator's pointer tracks the worker's task (the child's notify
/// events call this on the creator's copy).
pub fn mirror_status(creator: &mut SessionStore, id: &str, status: &str) -> StoreResult<()> {
    if find(creator, id)?.is_some() {
        append_event(
            creator,
            id,
            TaskEvent::Pointer {
                status: status.to_owned(),
            },
        )
    } else {
        Ok(())
    }
}

pub fn note(store: &mut SessionStore, id: &str, text: &str) -> StoreResult<()> {
    append_event(
        store,
        id,
        TaskEvent::Note {
            text: text.to_owned(),
        },
    )
}
