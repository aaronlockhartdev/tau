use super::*;

use serde_json::json;

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
    assert_eq!(c.task, "task-1");
    assert_eq!(c.status, STATUS_IN_PROGRESS);
    assert_eq!(c.current_step.as_ref().unwrap().text, "first");
    assert_eq!(c.next_action, "complete: first (expected: a.md)");
    assert_eq!(c.gaps.len(), 1);
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
    assert_eq!(c.current_step.as_ref().unwrap().text, "second");
    assert!(c.gaps.is_empty());
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
