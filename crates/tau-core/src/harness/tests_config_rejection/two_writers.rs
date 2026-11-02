//! The two writers over one session file (#384): the closed session's
//! `session_set_model` opens its own store to append its record, and the open
//! mints its facts through the store the agent is built on. The store-level
//! collision this shape used to mint is pinned in `diagnostics::tests`; these
//! legs pin the product path — the commands, in both orders — leave one chain:
//! every line on the branch, ids unique, the break stated once.

use super::*;

/// Every diagnostic code on the branch, in branch order, and the ids — the
/// chain's witness.
fn chain(cwd: &Path, session: &str) -> (Vec<String>, Vec<String>, usize) {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    let branch = store.branch().unwrap();
    let total = store.entries_range(0, usize::MAX).unwrap().len();
    let codes = branch
        .iter()
        .filter(|e| e.kind == EntryKind::Diagnostic)
        .map(|e| {
            e.payload
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let ids = branch.iter().map(|e| e.id.clone()).collect();
    (codes, ids, total)
}

fn assert_one_chain(cwd: &Path, session: &str, expected_codes: &[&str]) {
    let (codes, ids, total) = chain(cwd, session);
    assert_eq!(
        codes, expected_codes,
        "the branch carries every diagnostic the file holds"
    );
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "no id is minted twice: {ids:?}");
    assert_eq!(
        ids.len(),
        total,
        "every line of the file is on the branch — none stranded"
    );
}

#[tokio::test]
async fn a_model_record_written_on_a_closed_session_and_the_open_fact_share_one_chain() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &broken_layer("5m")).await;
    // Writer one: the launch under the broken layer mints the config fact.
    let sid = new_session(&core, &ws);
    core.dispatch(Command::SessionClose {
        session: sid.clone(),
    })
    .unwrap();
    // Writer two: the closed arm opens its own store and appends.
    core.dispatch(Command::SessionSetModel {
        session: sid.clone(),
        model: "dev/model".into(),
    })
    .unwrap();
    core.dispatch(Command::SessionOpen {
        session: sid.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionClose {
        session: sid.clone(),
    })
    .unwrap();
    let cwd = PathBuf::from(&ws.cwd);
    assert_eq!(
        config_records(&cwd, &sid).len(),
        1,
        "one unfixed break is stated once, the model record beside it"
    );
    assert_one_chain(&cwd, &sid, &["config_rejected", "model_changed"]);
}

#[tokio::test]
async fn a_model_record_written_while_live_and_the_opens_fact_share_one_chain() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, ws) = core_over(&system, &project, &broken_layer("5m")).await;
    let sid = new_session(&core, &ws);
    // The live arm: the record rides the agent's own store, not a second one.
    core.dispatch(Command::SessionSetModel {
        session: sid.clone(),
        model: "dev/model".into(),
    })
    .unwrap();
    core.dispatch(Command::SessionClose {
        session: sid.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionOpen {
        session: sid.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionClose {
        session: sid.clone(),
    })
    .unwrap();
    let cwd = PathBuf::from(&ws.cwd);
    assert_eq!(
        config_records(&cwd, &sid).len(),
        1,
        "the reopen's fold reads the break and states it no more"
    );
    assert_one_chain(&cwd, &sid, &["config_rejected", "model_changed"]);
}
