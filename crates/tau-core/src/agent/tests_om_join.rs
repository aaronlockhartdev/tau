//! The late-commit double-observe guard (#402): a cycle that commits during
//! the D13.1 join moves the settle's plan, and a commit whose range
//! intersects a durable chunk is dropped, not appended.

use super::*;

use crate::agent::testkit::*;

/// The settle plans from the post-commit state: the state clone is taken
/// again after the join, so raw a late commit already covered is never
/// observed a second time.
#[tokio::test]
async fn a_cycle_committing_during_the_join_moves_the_settles_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // Five entries of 250 tokens (1000 chars); entry 4 is the raw the late
    // commit will cover.
    let mut parent: Option<String> = None;
    for i in 1..=5 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": format!("B{i} {}", "x".repeat(995)), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    // Activation 400: the post-join window (entry 5 + the turn's user entry,
    // ~255) sits under it; the pre-join window (entries 4, 5 + the turn's,
    // ~505) clears it. The 250-increment floor lets one promote drain both
    // durable chunks (#392's target math).
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 400,
        reflect_threshold: 100_000,
        buffer_increment: 250,
        buffer_tokens: None,
        retries: crate::config::OmRetries::default(),
    };
    // The durable record: a chunk covering entries 1..3; the reopen restores
    // the buffer cursor to its end (#392).
    let record = crate::om::OmRecord {
        buffered_chunks: vec![crate::om_integration::BufferedChunk {
            range: ("00000001".into(), "00000003".into()),
            last_ts: 300,
            text: "<observations>CYC-A</observations>".into(),
            tokens: 750,
        }],
        ..Default::default()
    };
    let provider = Arc::new(ScriptedProvider::new(vec![sse("a1", &[])]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(&cfg, record)),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    // The in-flight cycle: the real commit point, released the moment the
    // join takes the handle — so the commit lands after the pre-join clone.
    // The boundary rides far past the branch's size: the turn-end trigger
    // spawns no second cycle after this one.
    let inner2 = agent.inner.clone();
    let e4 = agent
        .inner
        .lock()
        .store
        .branch()
        .unwrap()
        .iter()
        .find(|e| e.id == "00000004")
        .cloned()
        .expect("entry 4 is on the branch");
    agent.inner.lock().om_inflight = Some(tokio::spawn(async move {
        for _ in 0..100_000 {
            if inner2.lock().om_inflight.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        crate::om_integration::background::commit_chunk(
            &inner2,
            &[e4],
            ("00000004".to_owned(), "00000004".to_owned()),
            250,
            "<observations>CYC-B</observations>".to_owned(),
            100_000,
            "test-model",
        );
    }));
    agent.send("go1", Lane::FollowUp);
    agent.process().await.unwrap();

    // The settle saw the committed chunk: the fresh window is entry 5 plus
    // the turn's user entry, under activation — no Observer call, the only
    // provider call is the main turn.
    assert_eq!(
        provider.user_contents.lock().len(),
        1,
        "the settle re-observed raw the late commit covered"
    );
    let state = agent
        .inner
        .lock()
        .om
        .clone()
        .expect("the om state is written back");
    // The settle's free promote drains both chunks — the durable one and the
    // late commit's (no wire call, #392) — and the cursor rides their
    // furthest end. A pre-join plan cannot see the late chunk, so it plans
    // an Observer over entry 4 (the extra call above) and the span's commit
    // stamps the cursor at entry 5.
    assert_eq!(
        state.record.cursor.as_ref().map(|c| c.entry_id.as_str()),
        Some("00000004"),
        "the cursor did not ride the drained chunks' furthest end"
    );
    // The late chunk rode the free promote: its observation stands in the
    // log, its raw never rides an Observer request.
    assert!(
        state.record.active_observations.contains("CYC-B"),
        "the late commit's observation did not reach the log"
    );
    assert!(state.record.buffered_chunks.is_empty());
}

/// A commit whose range intersects a durable chunk is a late re-observe of
/// the same raw: dropped, the interval stays consumed, the span is observed
/// once. A disjoint commit still lands.
/// A branch of eight entries with a durable chunk covering 3..7, plus a
/// fresh agent over it. The tempdir must outlive the agent.
fn overlap_fixture(dir: &tempfile::TempDir) -> AgentSession {
    let mut store = session_in(dir.path());
    let mut parent: Option<String> = None;
    for _ in 1..=8 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": "y".repeat(40), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let record = crate::om::OmRecord {
        buffered_chunks: vec![crate::om_integration::BufferedChunk {
            range: ("00000003".into(), "00000007".into()),
            last_ts: 700,
            text: "DURABLE".into(),
            tokens: 500,
        }],
        ..Default::default()
    };
    store
        .append(
            EntryKind::Om,
            serde_json::to_value(&record).unwrap(),
            parent.as_deref(),
        )
        .unwrap();
    AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: Arc::new(ScriptedProvider::new(vec![])),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &crate::config::Om::default(),
            crate::om::OmRecord::default(),
        )),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    })
}

/// The single entry 8 the commit batch carries.
fn entry8(agent: &AgentSession) -> crate::session::Entry {
    agent
        .inner
        .lock()
        .store
        .branch()
        .unwrap()
        .iter()
        .find(|e| e.id == "00000008")
        .cloned()
        .expect("entry 8 is on the branch")
}

#[test]
fn a_late_commit_that_intersects_a_durable_range_is_dropped() {
    // #402: a late commit over raw a durable chunk covers must not append a
    // second observation over the same span, and the drop must not touch the
    // file.
    let dir = tempfile::tempdir().unwrap();
    let agent = overlap_fixture(&dir);
    let e8 = entry8(&agent);
    crate::om_integration::background::commit_chunk(
        &agent.inner,
        &[e8],
        ("00000005".into(), "00000009".into()),
        100,
        "LATE".into(),
        10,
        "test-model",
    );
    let mut guard = agent.inner.lock();
    let loaded = crate::om_integration::OmState::load_record(&mut guard.store).unwrap();
    assert_eq!(
        loaded.buffered_chunks.len(),
        1,
        "an intersecting late commit appended a second observation over the same raw"
    );
    assert_eq!(loaded.buffered_chunks[0].text, "DURABLE");
    let om_entries = guard
        .store
        .branch()
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Om)
        .count();
    assert_eq!(om_entries, 1, "the dropped commit still wrote the file");
}

#[test]
fn a_disjoint_late_commit_still_lands() {
    // The drop is intersection, not the tail: a chunk past the durable
    // range's end commits and the live mirror rides it.
    let dir = tempfile::tempdir().unwrap();
    let agent = overlap_fixture(&dir);
    let e8 = entry8(&agent);
    crate::om_integration::background::commit_chunk(
        &agent.inner,
        &[e8],
        ("00000008".into(), "00000008".into()),
        10,
        "TAIL".into(),
        20,
        "test-model",
    );
    let mut guard = agent.inner.lock();
    let loaded = crate::om_integration::OmState::load_record(&mut guard.store).unwrap();
    assert_eq!(loaded.buffered_chunks.len(), 2);
    assert_eq!(loaded.buffered_chunks[1].text, "TAIL");
    assert_eq!(
        crate::om_integration::last_chunk_end(&guard.om.as_ref().unwrap().record.buffered_chunks,)
            .as_deref(),
        Some("00000008"),
        "the live mirror rides the committed chunk"
    );
}
