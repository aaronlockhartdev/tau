use super::*;

use crate::agent::testkit::*;

/// End-to-end OM (the ticket's acceptance bar): synthesized raw entries
/// cross the observe threshold, the canned Observer fills the log past
/// the reflect threshold, the canned Reflector rewrites the suffix —
/// and the continuation hint is injected exactly once across the run
/// (B2 at the agent level).
#[tokio::test]
async fn om_crosses_observe_and_reflect_and_the_hint_is_one_shot() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    // Real content accumulation: 5 synthesized entries cross the
    // 1000-token observe threshold (5 x 225 tokens).
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                "user",
                json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
    };
    let obs_text = format!(
        "<observations>obs {}</observations>",
        (0..900)
            .map(|i| format!("L{:04} data", i))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ref_text = format!(
        "<observations>condensed {}</observations>",
        (0..200)
            .map(|i| format!("c{:04}", i))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
        sse_json(&ref_text),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let state = agent
        .inner
        .lock()
        .unwrap()
        .om
        .clone()
        .expect("the om state is written back");
    // Observe: the log is non-empty and the cursor sits on turn 1's
    // last raw entry (the raw window for turn 2 is the new user entry).
    assert!(state.record.active_observations.contains("obs"));
    assert_eq!(state.record.cursor.unwrap().entry_id, "00000007");
    // Reflect: the tagged reflection committed its <observations>
    // content only, as generation 1.
    assert_eq!(state.record.generation, 1);
    assert!(state.record.active_observations.contains("condensed"));
    assert!(!state.record.active_observations.contains("<observations>"));
    // The continuation hint: exactly one assembled context in the whole
    // run carries it — the turn-2 assembly, right after the observe.
    let seen = provider.seen.lock().unwrap();
    let hints = seen
        .iter()
        .filter(|(ins, _)| {
            ins.as_deref()
                .is_some_and(|i| i.contains(crate::om::OBSERVATION_CONTINUATION_HINT))
        })
        .count();
    assert_eq!(hints, 1, "the hint is one-shot: {seen:?}");
}

/// The OM-run status hook (the app's om_status emitter): each run
/// reports its kind at start and `idle` at the end, in order.
#[tokio::test]
async fn om_runs_notify_the_status_hook_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                "user",
                json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
    };
    let obs_text = format!(
        "<observations>obs {}</observations>",
        (0..900)
            .map(|i| format!("L{:04} data", i))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ref_text = format!(
        "<observations>condensed {}</observations>",
        (0..200)
            .map(|i| format!("c{:04}", i))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
        sse_json(&ref_text),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    let kinds = Arc::new(Mutex::new(Vec::<String>::new()));
    let k = kinds.clone();
    agent.set_om_status_hook(Some(Arc::new(move |kind: &str| {
        k.lock().unwrap().push(kind.to_owned());
    })));
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();
    assert_eq!(
        *kinds.lock().unwrap(),
        vec!["observing", "idle", "reflecting", "idle"]
    );
}

/// A compacted-spawn-seeded record: the frozen prefix survives the
/// child's reflect byte-identical (ADR-0004), and the reflector prompt
/// carries the frozen-prefix marker.
#[tokio::test]
async fn a_compacted_seed_keeps_the_frozen_prefix_byte_identical_across_reflect() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                "user",
                json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
    };
    let obs_text = format!(
        "<observations>obs {}</observations>",
        (0..900)
            .map(|i| format!("L{:04} data", i))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ref_text = format!(
        "<observations>condensed {}</observations>",
        (0..200)
            .map(|i| format!("c{:04}", i))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
        sse_json(&ref_text),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord {
                frozen_prefix: "FROZEN PARENT LOG".into(),
                ..Default::default()
            },
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();

    let state = agent
        .inner
        .lock()
        .unwrap()
        .om
        .clone()
        .expect("the om state is written back");
    assert_eq!(state.record.frozen_prefix, "FROZEN PARENT LOG");
    assert_eq!(state.record.generation, 1);
    // The reflector call used the frozen variant.
    let seen = provider.seen.lock().unwrap();
    let reflect = seen
        .iter()
        .find(|(_, content)| {
            content
                .as_deref()
                .is_some_and(|c| c.contains("<frozen-prefix>"))
        })
        .expect("the reflector prompt carries the frozen-prefix marker");
    assert!(
        reflect.1.as_deref().unwrap().contains("FROZEN PARENT LOG"),
        "{reflect:?}"
    );
}

/// Spec §5.3: a session holding an active task re-injects the resume
/// contract after its OM compacts — the second turn's system prompt
/// carries it even though the raw window no longer does.
#[tokio::test]
async fn a_compaction_reinjects_the_active_task_resume_contract() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = session_in(dir.path());
    crate::task::create(
        &mut store,
        "task-1",
        "the long job",
        vec![crate::task::Step {
            text: "the step".into(),
            expected_output: "the artifact".into(),
            status: crate::task::StepStatus::Pending,
        }],
        vec![crate::task::Criterion {
            text: "the criterion".into(),
            status: crate::task::CriterionStatus::Pending,
        }],
    )
    .unwrap();
    crate::task::start(&mut store, "task-1").unwrap();
    // 1000-token observe threshold (5 x 225 tokens), like the sibling.
    let mut parent: Option<String> = None;
    for _ in 0..5 {
        let e = store
            .append(
                "user",
                json!({ "text": "a".repeat(900), "lane": "follow-up" }),
                parent.as_deref(),
            )
            .unwrap();
        parent = Some(e.id);
    }
    let cfg = crate::config::Om {
        om_model: String::new(),
        observe_threshold: 1000,
        reflect_threshold: 2000,
        buffer_increment: 230,
    };
    let obs_text = "<observations>observed work</observations>".to_owned();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("a1", &[]),
        sse_json(&obs_text),
        sse("a2", &[]),
    ]));
    let agent = AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: provider.clone(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: Some(crate::om_integration::OmState::from_config(
            &cfg,
            crate::om::OmRecord::default(),
        )),
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go1", Lane::FollowUp);
    agent.send("go2", Lane::FollowUp);
    agent.process().await.unwrap();
    // The active task's contract rides every assembly — turn 1 (before
    // any compaction) and turn 2 (after the observe compacted the raw
    // window, which no longer carries the task entries).
    let seen = provider.seen.lock().unwrap();
    // `seen` also holds the observer's own calls; the turn assemblies
    // are the ones built on the session's system prompt.
    let assemblies = seen
        .iter()
        .filter(|(ins, _)| ins.as_deref().is_some_and(|i| i.starts_with("be terse")))
        .count();
    assert_eq!(assemblies, 2, "two turn assemblies");
    for (ins, _) in seen
        .iter()
        .filter(|(ins, _)| ins.as_deref().is_some_and(|i| i.starts_with("be terse")))
    {
        let ins = ins.as_deref().unwrap();
        assert!(ins.contains("# Task (resume contract)"), "{ins}");
    }
}
