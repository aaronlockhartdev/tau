use super::*;

use crate::session::SessionStore;
use crate::subagent::testkit::*;

/// The concurrency cap is enforced; a child's tool set carries no
/// spawn tool (the depth cap is structural).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_caps() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _) = harness(
        dir.path(),
        vec![sse("x", &[])],
        vec![
            vec![
                sse("", &[notify_call("n1", "note", false, None, Some("user"))]),
                sse("", &[]),
                sse(
                    "",
                    &[notify_call(
                        "n2",
                        "done",
                        true,
                        Some(json!({"ok": true})),
                        None,
                    )],
                ),
                sse("", &[]),
            ],
            vec![
                sse(
                    "",
                    &[notify_call(
                        "n3",
                        "done",
                        true,
                        Some(json!({"ok": true})),
                        None,
                    )],
                ),
                sse("", &[]),
            ],
        ],
        SubAgents {
            max_depth: 1,
            max_concurrent: Some(1),
        },
    );
    let first = sup
        .spawn("general", "one", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    // Second spawn while the first holds its slot: refused.
    let err = sup
        .spawn("general", "two", Some(ContextMode::Fresh), None, "c1")
        .expect_err("second spawn while the first holds its slot");
    assert!(err.contains("concurrency cap"), "{err}");
    // The child's tool set: parent_notify in, subagent_spawn out.
    let tools = tools::child_tool_specs()
        .iter()
        .map(|t| t.name.clone())
        .collect::<Vec<_>>();
    assert!(tools.contains(&"parent_notify".to_owned()));
    assert!(!tools.contains(&"subagent_spawn".to_owned()));
    // Free the slot: the done child no longer counts. Wait for the
    // park first so the resume is deterministic (a message delivered
    // mid-brief-turn would be a steering, consumed by that turn).
    wait_for(|| {
        matches!(
            sup.state_info(&first.handle).unwrap().state,
            ChildState::Idle { .. }
        )
    });
    sup.message(&first.handle, Some("finish".into()), Lane::Steering)
        .unwrap();
    wait_for(|| {
        matches!(
            sup.state_info(&first.handle).unwrap().state,
            ChildState::Done { .. }
        )
    });
    let second = sup.spawn("general", "two", Some(ContextMode::Fresh), None, "c1");
    assert!(
        second.is_ok(),
        "a done child frees its slot: {:?}",
        second.err()
    );
    // Unknown agent types are refused, naming what is available.
    let err = sup
        .spawn("planner", "x", Some(ContextMode::Fresh), None, "c2")
        .expect_err("an unknown type is refused");
    assert!(err.contains("unknown agent type \"planner\""), "{err}");
    assert!(err.contains("general"), "{err}");
}

/// A `.md` type (spec §5.5) configures the child: its body is the
/// system prompt, its model the child's model, its tools a subset of
/// the child's default set.
#[tokio::test]
async fn a_md_type_configures_the_child_prompt_model_and_tools() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(project.join(".tau").join("agents")).unwrap();
    std::fs::write(
        project.join(".tau/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: reviews\ntools: read\nmodel: review-model\n---\nYou are a strict reviewer.\n",
    )
    .unwrap();
    let types = crate::agent_type::discover(None, &project);
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![sse("done", &[])]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        caps: SubAgents::default(),
        depth: 0,
        types,
        bridge: bridge as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
    });
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: crate::provider::canned(sse("ok", &[]).as_str()),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(parent);
    let spawned = sup
        .spawn("reviewer", "look", None, None, "c0")
        .expect("the discovered type spawns");
    // The type's body, not the session's prompt; the type's model, not
    // the session's; the subset, not the full child set.
    assert_eq!(spawned.agent.system_prompt(), "You are a strict reviewer.");
    assert_eq!(spawned.model, "review-model");
    assert_eq!(spawned.agent.tools().len(), 1);
    assert_eq!(spawned.agent.tools()[0].name, "read");
    // An omitted context_mode falls back to the type's default (fresh
    let children = sup.children.lock().unwrap();
    let child = children.get(&spawned.handle).unwrap();
    assert_eq!(child.context_mode, ContextMode::Fresh);
}

/// A `general` child inherits the session's prompt (spec §5.5) —
/// including the skill catalog that `build_live` appended as the last
/// layer (ticket #28).
#[tokio::test]
async fn a_general_child_inherits_the_catalog_carrying_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let skill = crate::skills::Skill {
        name: "alpha".into(),
        description: "does alpha".into(),
        location: dir.path().join(".agents/skills/alpha/SKILL.md"),
        model_invocation: true,
    };
    let catalog = crate::skills::catalog(&[skill]).unwrap();
    let prompt = format!("You are Tau, a coding agent.\n\n{catalog}");
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![vec![sse("done", &[])]],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory,
        model: "test-model".into(),
        system_prompt: prompt.clone(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: crate::agent_type::discover(None, dir.path()),
        bridge: bridge as Arc<dyn SubagentBridge>,
        driver: Arc::new(TestDriver),
    });
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: prompt.clone(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: crate::provider::canned(sse("ok", &[]).as_str()),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(parent);
    let spawned = sup
        .spawn("general", "look", None, None, "c0")
        .expect("general always spawns");
    // The session's prompt verbatim — the catalog rides along with it.
    assert_eq!(spawned.agent.system_prompt(), prompt);
}
/// `max_depth` is enforced at spawn: a session at the cap refuses to
/// spawn, and `max_depth: 0` disables spawning outright (the
/// structural child-carries-no-supervisor rule already bounds v0 depth
/// to 1, so depth 0 is the only reachable check in a live tree).
#[tokio::test]
async fn the_depth_cap() {
    let dir = tempfile::tempdir().unwrap();
    let sup = |depth: u32, max_depth: u32| -> Arc<Supervisor> {
        let bridge = Arc::new(TestBridge::default());
        let factory = Arc::new(CannedFactory {
            scripts: vec![],
            delays: vec![],
            created: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        Supervisor::new(SupervisorParams {
            parent_session: "parent".into(),
            cwd: dir.path().to_path_buf(),
            provider: factory,
            model: "test-model".into(),
            system_prompt: "be terse".into(),
            om: Om::default(),
            om_model: String::new(),
            tool_batch_on_force: ToolBatchPolicy::Complete,
            turn: TurnConfig::default(),
            caps: SubAgents {
                max_depth,
                max_concurrent: None,
            },
            types: vec![crate::agent_type::builtin_general()],
            depth,
            bridge: bridge as Arc<dyn SubagentBridge>,
            driver: Arc::new(TestDriver),
        })
    };
    // At the cap: refused with a clear diagnostic.
    let at_cap = sup(1, 1);
    let err = at_cap
        .spawn("general", "x", Some(ContextMode::Fresh), None, "c0")
        .expect_err("a session at the depth cap cannot spawn");
    assert!(err.contains("depth cap"), "{err}");
    // max_depth 0 disables spawning for a top-level session.
    let zero = sup(0, 0);
    let err = zero
        .spawn("general", "x", Some(ContextMode::Fresh), None, "c0")
        .expect_err("max_depth 0 disables spawning");
    assert!(err.contains("depth cap"), "{err}");
    // Below the cap the depth check passes (spawn then fails only on
    // the missing parent attachment, not on depth).
    let below = sup(0, 1);
    let err = below
        .spawn("general", "x", Some(ContextMode::Fresh), None, "c0")
        .expect_err("no parent attached in this bare harness");
    assert!(!err.contains("depth cap"), "{err}");
}

/// The drive-generation guard (review N1): stop + resume landing in the
/// window after the in-flight `drive()` returns must not let the old
/// drive consume the child's nudge budget and run rounds alongside the
/// fresh drive. A notify-gated driver makes the window deterministic:
/// both drives are in flight when the test releases them at once — the
/// fresh drive legitimately consumes the nudge budget (its own turn);
/// without the guard the superseded drive would race it for the budget
/// and its loser marks the child `failed` (nudge exhausted).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_resume_in_the_drive_window_does_not_double_drive() {
    /// Each `drive()` blocks until the test's notify fires.
    struct NotifyingDriver {
        notify: Arc<tokio::sync::Notify>,
        calls: AtomicUsize,
    }
    impl ChildDriver for NotifyingDriver {
        fn drive(&self, _session: &str, _agent: &Arc<AgentSession>) -> BoxedDrive {
            let notify = Arc::clone(&self.notify);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                notify.notified().await;
                Ok(())
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(TestBridge::default());
    let factory = Arc::new(CannedFactory {
        scripts: vec![],
        delays: vec![],
        created: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let driver = Arc::new(NotifyingDriver {
        notify: Arc::new(tokio::sync::Notify::new()),
        calls: AtomicUsize::new(0),
    });
    let sup = Supervisor::new(SupervisorParams {
        parent_session: "parent".into(),
        cwd: dir.path().to_path_buf(),
        provider: factory as Arc<dyn ChildProviderFactory>,
        model: "test-model".into(),
        system_prompt: "be terse".into(),
        om: Om::default(),
        om_model: String::new(),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        caps: SubAgents::default(),
        depth: 0,
        types: vec![crate::agent_type::builtin_general()],
        bridge: bridge as Arc<dyn SubagentBridge>,
        driver: Arc::clone(&driver) as Arc<dyn ChildDriver>,
    });
    let mut store = SessionStore::for_workspace(dir.path(), "parent");
    store.create().unwrap();
    let parent = Arc::new(AgentSession::new(SessionParams {
        store,
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider: Arc::new(ScriptedProvider::new(vec![sse("", &[])])),
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    }));
    sup.attach_parent(parent);

    // Drive #1 is in flight; stop + resume then spawn drive #2, which
    // is in flight too — the window opens when the notify releases
    // both rounds at once.
    let spawned = sup
        .spawn("general", "work", Some(ContextMode::Fresh), None, "c0")
        .unwrap();
    wait_for(|| driver.calls.load(Ordering::SeqCst) == 1);
    sup.stop(&spawned.handle, StoppedBy::User).unwrap();
    sup.message(&spawned.handle, Some("go".into()), Lane::Steering)
        .unwrap();
    wait_for(|| driver.calls.load(Ordering::SeqCst) == 2);
    driver.notify.notify_waiters();
    // The superseded drive ends here, on the generation mismatch.
    spawned.drive.await.unwrap();
    assert!(
        !matches!(
            sup.state_info(&spawned.handle).unwrap().state,
            ChildState::Failed { .. }
        ),
        "the superseded drive must not exhaust the nudge budget"
    );
    let mut store = SessionStore::for_workspace(dir.path(), &spawned.session_id);
    store.open().unwrap();
    let entries = store.entries_range(0, usize::MAX).unwrap();
    let nudges = entries
        .iter()
        .filter(|e| {
            e.kind == "system"
                && e.payload
                    .get("note")
                    .and_then(|n| n.as_str())
                    .map(|n| n.starts_with("Nudge"))
                    .unwrap_or(false)
        })
        .count();
    assert_eq!(
        nudges, 1,
        "exactly the fresh drive's nudge; the superseded drive added none"
    );
    // Cleanup: the fresh drive's next round is gated; stop the child.
    sup.stop(&spawned.handle, StoppedBy::User).unwrap();
}
