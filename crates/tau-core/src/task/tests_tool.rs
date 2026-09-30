use super::*;
use serde_json::json;

use crate::session::SessionStore;

fn criterion(text: &str) -> Criterion {
    Criterion {
        text: text.into(),
        status: CriterionStatus::Pending,
    }
}

fn session_in(dir: &std::path::Path, id: &str) -> SessionStore {
    let mut store = SessionStore::for_workspace(dir, id);
    store.create().unwrap();
    store
}

/// A created task with one criterion: the shared fixture for the
/// transition tests.
fn with_task(store: &mut SessionStore) {
    create(store, "task-1", "ship it", Vec::new(), vec![criterion("it builds")]).unwrap();
}

#[test]
fn create_reports_the_id_steps_and_criteria() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    let out = tool_call(
        &mut store,
        "task_create",
        &json!({
            "title": "ship it",
            "steps": [{ "text": "draft", "expected_output": "docs/draft.md" }],
            "criteria": ["it builds"],
        }),
    );
    assert_eq!(out, "created task-1: ship it (1 steps, 1 criteria)");
}

#[test]
fn create_missing_title_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_create", &json!({ "steps": [] })),
        "task_create: missing \"title\""
    );
}

#[test]
fn create_step_without_text_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(
            &mut store,
            "task_create",
            &json!({ "title": "t", "steps": [{ "expected_output": "x" }] })
        ),
        "task_create: step 0 is missing \"text\""
    );
}

#[test]
fn create_step_without_expected_output_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(
            &mut store,
            "task_create",
            &json!({ "title": "t", "steps": [{ "text": "draft" }] })
        ),
        "task_create: step 0 (\"draft\") is missing \"expected_output\""
    );
}

#[test]
fn create_non_string_criterion_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(
            &mut store,
            "task_create",
            &json!({ "title": "t", "criteria": [42] })
        ),
        "task_create: criterion 0 is not a string"
    );
}

#[test]
fn start_missing_task_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_start", &json!({})),
        "task_start: missing \"task\""
    );
}

#[test]
fn start_of_an_unknown_task_reports_the_store_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_start", &json!({ "task": "task-1" })),
        "task_start: task task-1: not found in this session"
    );
}

#[test]
fn evidence_missing_fields_are_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_evidence", &json!({ "criterion": "c" })),
        "task_evidence: missing \"task\""
    );
    assert_eq!(
        tool_call(&mut store, "task_evidence", &json!({ "task": "task-1" })),
        "task_evidence: missing \"criterion\""
    );
    assert_eq!(
        tool_call(
            &mut store,
            "task_evidence",
            &json!({ "task": "task-1", "criterion": "c" })
        ),
        "task_evidence: missing \"summary\""
    );
}

#[test]
fn evidence_of_an_unknown_task_reports_the_store_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(
            &mut store,
            "task_evidence",
            &json!({ "task": "task-1", "criterion": "c", "summary": "s" })
        ),
        "task_evidence: task task-1: not found in this session"
    );
}

#[test]
fn block_missing_fields_are_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_block", &json!({ "reason": "r" })),
        "task_block: missing \"task\""
    );
    assert_eq!(
        tool_call(&mut store, "task_block", &json!({ "task": "task-1" })),
        "task_block: missing \"reason\""
    );
}

#[test]
fn finish_missing_task_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_finish", &json!({})),
        "task_finish: missing \"task\""
    );
}

#[test]
fn cancel_missing_task_is_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_cancel", &json!({})),
        "task_cancel: missing \"task\""
    );
}

#[test]
fn cancel_of_an_unknown_task_reports_the_store_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_cancel", &json!({ "task": "task-9" })),
        "task_cancel: task task-9: not found in this session"
    );
}

#[test]
fn an_unknown_tool_name_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    assert_eq!(
        tool_call(&mut store, "task_teleport", &json!({})),
        "unknown task tool task_teleport"
    );
}

#[test]
fn the_full_lifecycle_runs_create_start_evidence_finish() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    tool_call(
        &mut store,
        "task_create",
        &json!({ "title": "ship it", "criteria": ["it builds"] }),
    );
    assert!(tool_call(&mut store, "task_start", &json!({ "task": "task-1" }))
        .contains("in_progress"));
    assert!(tool_call(
        &mut store,
        "task_evidence",
        &json!({ "task": "task-1", "criterion": "it builds", "summary": "green" })
    )
    .contains("task-1"));
    assert!(tool_call(&mut store, "task_finish", &json!({ "task": "task-1" }))
        .contains("done"));
}

#[test]
fn finish_without_passing_evidence_is_refused_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    with_task(&mut store);
    tool_call(&mut store, "task_start", &json!({ "task": "task-1" }));
    let refused = tool_call(&mut store, "task_finish", &json!({ "task": "task-1" }));
    assert!(refused.contains("task-1"), "{refused}");
    assert!(
        !refused.contains("done"),
        "an unmet criterion must not finish: {refused}"
    );
    let forced = tool_call(
        &mut store,
        "task_finish",
        &json!({ "task": "task-1", "force": true, "reason": "ship it anyway" }),
    );
    assert!(forced.contains("done"), "{forced}");
}

#[test]
fn note_on_an_unknown_task_is_rejected() {
    // The dispatch boundary's TaskUpdate: a note for a stale id must not
    // append an orphan entry and report success.
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path(), "s1");
    let err = note(&mut store, "task-1", "hello").unwrap_err();
    assert_eq!(err, "task task-1: not found in this session");
}
