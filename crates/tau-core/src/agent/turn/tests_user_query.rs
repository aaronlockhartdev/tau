//! The user-query invariant (#334): an assembled request answers a query, so
//! it carries at least one `role: user` item. The newest user entry of the
//! active branch anchors a window whose tail carries none; the seam refuses an
//! assembly that has no user item at all.

use super::*;
use crate::agent::SessionParams;
use crate::agent::testkit::{ScriptedProvider, session_in, sse};
use crate::session::SessionStore;
use serde_json::json;
use std::sync::Arc;

fn append(store: &mut SessionStore, kind: EntryKind, payload: serde_json::Value) -> String {
    let parent = store.leaf().unwrap().map(|e| e.id);
    store.append(kind, payload, parent.as_deref()).unwrap().id
}

/// An assistant entry whose text is empty and whose call is answered by a tool
/// entry: the tool-run shape the §4 cut removes from the window head.
fn tool_run(store: &mut SessionStore) -> String {
    let assistant = append(
        store,
        EntryKind::Assistant,
        json!({ "text": "", "calls": [{ "id": "i1", "call_id": "c1", "name": "bash", "arguments": "{}" }] }),
    );
    append(
        store,
        EntryKind::Tool,
        json!({ "call_id": "c1", "name": "bash", "args": {}, "output": "ok", "status": "success" }),
    );
    assistant
}

fn om_state(cursor: Option<&str>) -> crate::om_integration::OmState {
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
        buffer_tokens: 0,
        retries: crate::config::OmRetries::default(),
    };
    crate::om_integration::OmState::from_config(
        &cfg,
        crate::om::OmRecord {
            cursor: cursor.map(|entry_id| crate::om::Cursor {
                entry_id: entry_id.to_owned(),
                timestamp: 1,
            }),
            active_observations: crate::om::wrap_in_observation_group("the log", "1:2", "a1", None),
            ..Default::default()
        },
    )
}

fn agent_with(
    store: SessionStore,
    dir: &std::path::Path,
    provider: Arc<ScriptedProvider>,
    om: Option<crate::om_integration::OmState>,
) -> Arc<AgentSession> {
    Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: crate::tools::tool_specs(),
        cwd: dir.to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om,
        om_model: String::new(),
        subagents: None,
        child: None,
    }))
}

fn user_contents(items: &[InputEntry]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            InputEntry::Message(m) if m.role == "user" => Some(m.content.clone()),
            _ => None,
        })
        .collect()
}

/// The shape that drew a provider 400: compaction advanced the cursor past the
/// newest user entry into a tool run, the §4 cut dropped that run, so the
/// unobserved tail carries no user entry. The newest user entry leads the
/// request instead, ahead of the observation items.
#[tokio::test]
async fn an_observed_brief_anchors_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    let brief = append(
        &mut store,
        EntryKind::User,
        json!({ "text": "the brief", "lane": "follow-up" }),
    );
    let assistant = tool_run(&mut store);
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    let agent = agent_with(
        store,
        dir.path(),
        provider.clone(),
        Some(om_state(Some(&assistant))),
    );
    let assembled = agent.assemble(None).expect("the assembly reads");
    let users = user_contents(&assembled.items);
    assert_eq!(
        users.first().map(String::as_str),
        Some("the brief"),
        "the anchor leads: {users:?}"
    );
    assert_eq!(
        users.iter().filter(|c| *c == "the brief").count(),
        1,
        "the anchor appears exactly once: {users:?}"
    );
    let om_prefix = &assembled.items[..assembled.om_items];
    assert!(
        om_prefix.iter().all(|item| matches!(
            item,
            InputEntry::Message(m) if matches!(m.role.as_str(), "user" | "system")
        )),
        "the anchored prefix is the anchor plus the observation items"
    );
    assert_eq!(
        user_contents(&assembled.items[assembled.om_items..]),
        Vec::<String>::new(),
        "the tail is the tool run, so only the anchor answers the query"
    );
    assert!(assembled.has_user_query());
    // The ENTRY is pinned, not the RANGE: the cursor stays where compaction
    // left it, so the observed tool run is not un-observed by the anchor.
    let state = agent.inner.lock().om.clone().expect("the om state");
    assert_eq!(
        state.record.cursor.as_ref().map(|c| c.entry_id.as_str()),
        Some(assistant.as_str()),
        "the cursor does not retreat to the anchor ({brief})"
    );
}

/// The cursor behind the newest user entry: that entry is in the tail, so it is
/// the request's query and nothing is prepended. An older user entry stays
/// observed and is not re-anchored.
#[tokio::test]
async fn a_user_entry_in_the_tail_is_the_query_and_is_not_duplicated() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    append(
        &mut store,
        EntryKind::User,
        json!({ "text": "the brief", "lane": "follow-up" }),
    );
    let assistant = append(
        &mut store,
        EntryKind::Assistant,
        json!({ "text": "on it", "calls": [] }),
    );
    append(
        &mut store,
        EntryKind::User,
        json!({ "text": "steer", "lane": "steering" }),
    );
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    let agent = agent_with(
        store,
        dir.path(),
        provider.clone(),
        Some(om_state(Some(&assistant))),
    );
    let assembled = agent.assemble(None).expect("the assembly reads");
    let users = user_contents(&assembled.items);
    assert_eq!(
        users.last().map(String::as_str),
        Some("steer"),
        "the tail's user entry answers the query: {users:?}"
    );
    assert_eq!(
        users.iter().filter(|c| *c == "the brief").count(),
        0,
        "an observed user entry is not re-anchored: {users:?}"
    );
    assert!(
        matches!(&assembled.items[0], InputEntry::Message(m) if m.role == "system"),
        "nothing is prepended: the request opens on the observation items"
    );
    assert_eq!(
        assembled.om_items, 2,
        "the observation group plus the hint, no anchor"
    );
}

/// A branch that carries no user entry at all is not a request: the seam
/// refuses it with a named error and a recorded fact, and the provider is never
/// called — the guard is the last line, not a filler string the model has to
/// explain.
#[tokio::test]
async fn an_assembly_with_no_user_entry_is_refused_before_the_wire() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    tool_run(&mut store);
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    let agent = agent_with(store, dir.path(), provider.clone(), None);
    let err = agent
        .run_turn()
        .await
        .expect_err("a request with no user query is refused");
    assert!(
        matches!(err, AgentError::MissingUserQuery),
        "expected the named refusal, saw {err:?}"
    );
    assert_eq!(
        provider.models.lock().len(),
        0,
        "the refused call never went out"
    );
    let mut read = SessionStore::for_workspace(dir.path(), "s1");
    read.open().unwrap();
    let recorded: Vec<Diagnostic> = read
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Diagnostic)
        .filter_map(|e| Diagnostic::from_value(&e.payload))
        .collect();
    assert!(
        recorded
            .iter()
            .any(|d| matches!(d, Diagnostic::MissingUserQuery { .. })),
        "the refusal leaves a fact: {recorded:?}"
    );
}

/// A user entry whose payload carries no text yields no user item: the
/// predicate reads the assembled items, not the entry kinds.
#[test]
fn an_empty_user_payload_owes_no_query() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    append(&mut store, EntryKind::User, json!({ "lane": "follow-up" }));
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    let agent = agent_with(store, dir.path(), provider.clone(), None);
    let assembled = agent.assemble(None).expect("the assembly reads");
    assert!(!assembled.has_user_query(), "no text, no query");
}
