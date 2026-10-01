//! The turn-end sequence (R4) driven through `settle_turn` — the actual
//! turn-end entry point, not just `branch_entries`.

use super::*;

#[tokio::test]
async fn settle_turn_terminates_on_a_cyclic_chain() {
    // Duplicate ids make the parent chain cyclic (corruption): the branch
    // walk inside settle_turn must terminate, not clone entries forever
    // (the 80 GB runaway's amplifier).
    let dir = tempfile::tempdir().unwrap();
    let mut store = SessionStore::for_workspace(dir.path(), "s1");
    store.create().unwrap();
    let mut a: Entry = serde_json::from_str(
        r#"{"id":"00000001","parentId":"00000002","timestamp":1,"type":"user","payload":{"text":"a"}}"#,
    )
    .unwrap();
    let mut b: Entry = serde_json::from_str(
        r#"{"id":"00000002","parentId":"00000001","timestamp":2,"type":"user","payload":{"text":"b"}}"#,
    )
    .unwrap();
    a.crc = Some(a.compute_crc());
    b.crc = Some(b.compute_crc());
    let header = std::fs::read_to_string(store.path()).unwrap();
    std::fs::write(
        store.path(),
        format!(
            "{}{}\n{}\n",
            header,
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        ),
    )
    .unwrap();
    // The file is valid (CRCs check) and cyclic (a ⇄ b).
    store.open().unwrap();

    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let provider = crate::provider::canned("the observer never runs at this size");
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    // Terminates with the default record (nothing to observe) — the point
    // is that the walk ends, not what the plan picks.
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn settle_turn_runs_the_observe_pass_and_commits() {
    let dir = tempfile::tempdir().unwrap();
    // ~50k chars per entry: the unobserved raw clears the observe
    // threshold (30k tokens), so the pass plans an Observe.
    let mut store = store_with_text_entries(dir.path(), 3, 50_000);
    let mut state = OmState::from_config(&crate::config::Om::default(), OmRecord::default());
    let provider = crate::provider::canned(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"<observations>user is setting up a workbench</observations>\"}\n\ndata: [DONE]\n\n",
    );
    let mut with_store =
        |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), OmError>| f(&mut store);
    state
        .settle_turn(&mut with_store, &provider, "om-model", None)
        .await
        .unwrap();
    // The observation is in the record, the cursor advanced, and the
    // record persisted to the session file.
    assert!(state.record.active_observations.contains("workbench"));
    assert_eq!(state.record.cursor.as_ref().unwrap().entry_id, "00000003");
    assert!(
        OmState::load_record(&mut store)
            .unwrap()
            .active_observations
            .contains("workbench")
    );
}
