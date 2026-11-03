//! The late-commit double-observe guard (#402): a cycle that commits during
//! the D13.1 join moves the settle's plan, and a commit whose range
//! intersects a durable chunk is dropped, not appended.

use super::*;

use crate::agent::testkit::*;

/// The settle plans from the post-commit state: the state clone is taken
/// again after the join, so raw a late commit already covered is never
/// observed a second time.
/// The in-flight cycle for the join leg: a real `commit_chunk` of entry 4,
/// released the moment the join takes the handle. The boundary rides far
/// past the branch's size: the turn-end trigger spawns no second cycle.
fn spawn_late_commit(
    inner2: std::sync::Arc<parking_lot::Mutex<crate::agent::Inner>>,
    e4: crate::session::Entry,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // The spin is a safety valve, not the mechanism. If it ever exhausts,
        // the commit would land before the join — and the leg would pass
        // vacuously — so the task asserts the join took the handle first. A
        // task panic surfaces as a `JoinError`: the join reports `timed_out`,
        // the settle plans from the pre-clone state, and the leg fails red
        // with its own #402 message.
        let mut released = false;
        for _ in 0..100_000 {
            if inner2.lock().om_inflight.is_none() {
                released = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(released, "the join must take the handle before the commit");
        crate::om_integration::background::commit_chunk(
            &inner2,
            &[e4],
            ("00000004".to_owned(), "00000004".to_owned()),
            250,
            "<observations>CYC-B</observations>".to_owned(),
            100_000,
            "test-model",
        );
    })
}

// `current_thread` is the leg's stage: the join's `timeout` is the main task's
// first Pending, so the runtime polls the cycle task, which commits
// synchronously, and the join resolves on its completion — the commit lands
// between the pre-join clone and the plan with no timing dependency.
#[tokio::test(flavor = "current_thread")]
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
    let inner2 = agent.inner.clone();
    agent.inner.lock().om_inflight = Some(spawn_late_commit(inner2, e4));
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

/// The settle's Observe advances the observation cursor without moving the
/// buffer cursor. A cycle planning from the buffer cursor alone pays again
/// for the span the settle just paid for — and with no durable chunk to
/// intersect, #402's commit dedup cannot see the duplicate. The cycle's
/// candidates are raw past BOTH cursors.
#[tokio::test]
async fn the_cycle_skips_raw_the_observation_cursor_has_covered() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // Eight entries of 250 tokens: entry k carries MARK-k, so the Observer
    // transcript names exactly the span the call was shown.
    let mut parent: Option<String> = None;
    for k in 1..=8 {
        let e = store
            .append(
                EntryKind::User,
                serde_json::json!({ "text": format!("MARK-{k} {}", "y".repeat(992)), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    // The durable record: the settle observed entries 1..4; nothing is
    // buffered, so the buffer cursor is None and a buffer-cursor-only plan
    // would hand the Observer the whole branch.
    let record = crate::om::OmRecord {
        cursor: Some(crate::om::Cursor {
            entry_id: "00000004".into(),
            timestamp: 400,
        }),
        ..Default::default()
    };
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 400,
        reflect_threshold: 100_000,
        buffer_increment: 300,
        buffer_tokens: None,
        retries: crate::config::OmRetries::default(),
    };
    let provider = Arc::new(ScriptedProvider::new(vec![sse(
        "<observations>POST</observations>",
        &[],
    )]));
    // The cycle reloads the record from the file ("the file wins"), so the
    // record has to be durable, not just the in-memory state.
    let om_state = crate::om_integration::OmState::from_config(&cfg, record);
    om_state.save(&mut store).unwrap();
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
        om: Some(om_state),
        om_route: crate::agent::OmRoute::Session,
        subagents: None,
        child: None,
    });
    let inner2 = agent.inner.clone();
    let config = agent.inner.lock().om.as_ref().unwrap().config.clone();
    let route: crate::provider::TurnProviderRef = provider.clone();
    agent.inner.lock().om_inflight = Some(tokio::spawn(
        crate::om_integration::background::buffer_cycle(
            inner2,
            config,
            route,
            "test-model".into(),
            300,
        ),
    ));
    let handle = agent
        .inner
        .lock()
        .om_inflight
        .take()
        .expect("the cycle spawned");
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("the cycle completed")
        .expect("the cycle task ran");
    let transcripts = provider.user_contents.lock().clone();
    assert_eq!(transcripts.len(), 1, "the cycle served exactly one call");
    let shown = transcripts[0].join("\n");
    assert!(
        shown.contains("MARK-5"),
        "the cycle skipped raw past both cursors: {shown}"
    );
    for k in 1..=4 {
        assert!(
            !shown.contains(&format!("MARK-{k}")),
            "the cycle re-showed the observed MARK-{k}"
        );
    }
}

#[test]
/// A commit whose range intersects a durable chunk is a late re-observe of
/// the same raw: dropped, the interval stays consumed, the span is observed
/// once.
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
