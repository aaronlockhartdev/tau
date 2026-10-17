//! The plan-file substrate's harness wiring (ticket #164, T1): the
//! session start seeds the file and injects its path, the plan watcher
//! keeps the parsed state fresh, and the assembly re-injects the state
//! at each turn boundary.

use super::*;
use crate::harness::testkit::*;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A scripted provider that records each request's instructions — the
/// assertion surface for what the assembly injected.
struct Capturing {
    bodies: Vec<String>,
    index: AtomicUsize,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

impl provider::TurnProvider for Capturing {
    fn call<'a>(
        &self,
        request: &provider::ResponseRequest,
        sink: &'a mut dyn provider::TurnSink,
    ) -> provider::ProviderTurn<'a> {
        let body = self
            .bodies
            .get(self.index.fetch_add(1, Ordering::SeqCst))
            .cloned()
            .unwrap_or_else(|| crate::agent::testkit::sse("done", &[]));
        let instructions = request.instructions().map(str::to_owned);
        let seen = self.seen.clone();
        let inner = provider::canned(&body);
        let turn = inner.call(request, sink);
        Box::pin(async move {
            seen.lock().unwrap().push(instructions);
            turn.await
        })
    }
}

/// One turn, drained: the turn flag clears and the capture is non-empty.
async fn settle_turn(live: &Arc<LiveSession>, seen: &Arc<Mutex<Vec<Option<String>>>>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let idle = !live.turn.load(Ordering::SeqCst);
        let captured = !seen.lock().unwrap().is_empty();
        if idle && captured {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn never settled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `SessionNew` through the production root path.
fn new_root_session(core: &Arc<Core>, ws: &Workspace) -> tau_protocol::snapshot::SessionMeta {
    let out = core
        .dispatch(Command::SessionNew {
            workspace: ws.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap();
    match out {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    }
}

#[tokio::test]
async fn session_new_seeds_the_plan_file_and_injects_its_path() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);

    // Seeded at session start, at the design's path.
    let plan_file = crate::plan::path(&cwd, &meta.id);
    assert!(
        plan_file.is_file(),
        "the plan file is seeded: {plan_file:?}"
    );
    // Its exact path is in the system prompt.
    let live = core.live(&meta.id).unwrap();
    let prompt = live.agent.system_prompt();
    assert!(
        prompt.contains(&plan_file.display().to_string()),
        "the prompt names the plan file: {prompt}"
    );
    // The initial state is the parsed skeleton: no steps yet.
    assert_eq!(
        core.plan_state(&meta.id).as_deref(),
        Some("# Current plan state")
    );
}
/// N2: the production wiring path — `SessionNew` wires the plan-state hook
/// (`wire_events`), and the re-injection reaches the provider's request
/// end-to-end, with no manual hook wiring (the `manual_session` tests set
/// the hook by hand, duplicating `wire_events`).
#[tokio::test]
async fn session_new_reinjection_reaches_the_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let provider = Arc::new(Capturing {
        bodies: vec![crate::agent::testkit::sse("working", &[])],
        index: AtomicUsize::new(0),
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    let seen = provider.seen.clone();
    // The root-provider seam: `SessionNew` binds the capturing provider to
    // the root session, so the turn's request is captured end-to-end.
    let core = CoreBuilder::custom(providers())
        .with_root_provider(provider)
        .build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);

    core.dispatch(Command::MessageSend {
        session: meta.id.clone(),
        text: "go".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let live = core.live(&meta.id).unwrap();
    settle_turn(&live, &seen).await;

    let ins = seen
        .lock()
        .unwrap()
        .first()
        .and_then(Clone::clone)
        .expect("the request carries instructions");
    assert!(
        ins.contains("# Current plan state"),
        "the production wiring re-injects the plan state: {ins}"
    );
}

#[tokio::test]
async fn a_plan_file_change_updates_the_plan_state() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);
    let plan_file = crate::plan::path(&cwd, &meta.id);

    std::fs::write(
        &plan_file,
        "# Plan\n\n## Steps\n- [x] done step\n- [ ] open step\n\n## Criteria\nbuild passes\n",
    )
    .unwrap();

    // The watcher re-reads and parses; the state carries the change.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state = core.plan_state(&meta.id);
        let updated = state
            .as_deref()
            .is_some_and(|s| s.contains("- [x] done step") && s.contains("build passes"));
        if updated {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the plan state never picked up the change: {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// T5: the snapshot carries the parsed plan (the plan pane's seed data):
/// an unobserved session snapshots an empty plan, and the watcher's parse
/// lands in `live.plan`.
#[tokio::test]
async fn the_snapshot_carries_the_parsed_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);

    // The seeded skeleton: no steps.
    let snap = match core
        .dispatch(tau_protocol::Command::SessionSnapshot {
            session: meta.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot: {other:?}"),
    };
    assert!(snap.live.plan.steps.is_empty(), "seeded skeleton: no steps");

    // The model edits the plan; the watcher's parse lands in the snapshot.
    let plan_file = crate::plan::path(&cwd, &meta.id);
    std::fs::write(
        &plan_file,
        "# Plan\n\n## Steps\n- [x] done step\n- [ ] open step\n\n## Criteria\nbuild passes\n",
    )
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snap = match core
            .dispatch(tau_protocol::Command::SessionSnapshot {
                session: meta.id.clone(),
            })
            .unwrap()
        {
            CommandOutput::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot: {other:?}"),
        };
        if snap.live.plan.steps.len() == 2 {
            assert!(snap.live.plan.steps[0].done);
            assert!(!snap.live.plan.steps[1].done);
            assert_eq!(snap.live.plan.criteria, "build passes");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the snapshot never carried the parsed plan: {:?}",
            snap.live.plan
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// T5: a plan-file change emits `plan_changed` (the plan pane's live
/// update): the full parsed plan, the idempotent full-state replacement.
#[tokio::test]
async fn a_plan_file_change_emits_plan_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let events = collect_events(&core);
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);

    let plan_file = crate::plan::path(&cwd, &meta.id);
    std::fs::write(&plan_file, "## Steps\n- [x] done step\n- [ ] open step\n").unwrap();

    // The session-start refresh also emits (the empty skeleton); the
    // assertion keys on the edited plan's two steps.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let got = events.lock().unwrap().iter().rev().find_map(|e| match e {
            Event::PlanChanged {
                workspace: ws,
                session,
                plan,
            } if ws == &workspace.id && session == &meta.id => Some(plan.clone()),
            _ => None,
        });
        if got.as_ref().is_some_and(|p| p.steps.len() == 2) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no plan_changed with the edited plan within 5 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// N4: the session-start seed emits `plan_changed` (the empty skeleton), so
/// a client that connects mid-session still receives the plan pane's data.
#[tokio::test]
async fn a_new_session_emits_plan_changed_for_the_seed() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let events = collect_events(&core);
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);

    // The session-start refresh emits the seeded skeleton (no steps).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let got = events.lock().unwrap().iter().rev().find_map(|e| match e {
            Event::PlanChanged {
                workspace: ws,
                session,
                plan,
            } if ws == &workspace.id && session == &meta.id => Some(plan.clone()),
            _ => None,
        });
        if got.as_ref().is_some_and(|p| p.steps.is_empty()) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no seed plan_changed within 5 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_plan_state_is_reinjected_into_the_assembly() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let provider = Arc::new(Capturing {
        bodies: vec![crate::agent::testkit::sse("working", &[])],
        index: AtomicUsize::new(0),
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    let seen = provider.seen.clone();
    let live = manual_session(&core, &workspace, provider, TurnConfig::default());
    let sid = live.meta.lock().unwrap().id.clone();

    // The production shape: the core's watcher-parsed state.
    let core_weak = Arc::downgrade(&core);
    let hook_sid = sid.clone();
    live.agent.set_plan_hook(Some(Arc::new(move || {
        core_weak
            .upgrade()
            .and_then(|core| core.plan_state(&hook_sid))
    })));
    // The session-start observation: the seeded skeleton.
    core.refresh_plan(Path::new(&workspace.cwd), &sid);

    core.dispatch(Command::MessageSend {
        session: sid.clone(),
        text: "go".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    settle_turn(&live, &seen).await;

    let ins = seen
        .lock()
        .unwrap()
        .first()
        .and_then(Clone::clone)
        .expect("the request carries instructions");
    assert!(
        ins.contains("# Current plan state"),
        "the plan state is re-injected: {ins}"
    );
}

#[tokio::test]
async fn a_plan_change_between_turns_changes_the_injected_state() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let provider = Arc::new(Capturing {
        bodies: vec![crate::agent::testkit::sse("working", &[])],
        index: AtomicUsize::new(0),
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    let seen = provider.seen.clone();
    let live = manual_session(&core, &workspace, provider, TurnConfig::default());
    let sid = live.meta.lock().unwrap().id.clone();
    let core_weak = Arc::downgrade(&core);
    let hook_sid = sid.clone();
    live.agent.set_plan_hook(Some(Arc::new(move || {
        core_weak
            .upgrade()
            .and_then(|core| core.plan_state(&hook_sid))
    })));
    core.refresh_plan(Path::new(&workspace.cwd), &sid);
    let plan_file = crate::plan::path(&cwd, &sid);

    // Turn 1: the seeded skeleton (no steps).
    core.dispatch(Command::MessageSend {
        session: sid.clone(),
        text: "go".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    settle_turn(&live, &seen).await;
    let first = seen.lock().unwrap()[0]
        .clone()
        .expect("the first request carries instructions");
    assert!(!first.contains("done step"), "turn 1: {first}");

    // The model edits the plan; the watcher refreshes the state.
    std::fs::write(&plan_file, "## Steps\n- [x] done step\n").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let updated = core
            .plan_state(&sid)
            .as_deref()
            .is_some_and(|s| s.contains("done step"));
        if updated {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the watcher never refreshed the plan state"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Turn 2: the assembly carries the updated state.
    core.dispatch(Command::MessageSend {
        session: sid.clone(),
        text: "go again".into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let prev = seen.lock().unwrap().len();
    settle_turn(&live, &seen).await;
    let seen = seen.lock().unwrap();
    assert!(seen.len() > prev, "turn 2 never ran");
    let second = seen[prev]
        .clone()
        .expect("the second request carries instructions");
    assert!(
        second.contains("- [x] done step"),
        "turn 2 carries the updated plan: {second}"
    );
}

#[tokio::test]
async fn closing_a_session_drops_its_plan_state() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);
    assert!(core.plan_state(&meta.id).is_some());

    core.dispatch(Command::SessionClose {
        session: meta.id.clone(),
    })
    .unwrap();
    assert!(
        core.plan_state(&meta.id).is_none(),
        "a closed session keeps no plan state"
    );
}

#[tokio::test]
async fn a_spawned_child_prompt_names_its_own_plan_file() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);
    let live = core.live(&meta.id).unwrap();
    let sup = live
        .agent
        .subagents()
        .expect("the root session carries a supervisor");
    assert!(
        sup.core().is_some(),
        "production root: the supervisor carries the core"
    );

    let parent_plan = crate::plan::path(&cwd, &meta.id);
    let spawned = sup
        .spawn("general", "do the thing", None, "call-1")
        .unwrap();
    let child_plan = crate::plan::path(Path::new(&spawned.cwd), &spawned.session_id);

    // The child's file is seeded, its prompt names it, and the inherited
    // parent path is gone (replaced, not stacked).
    assert!(child_plan.is_file(), "the child's plan file is seeded");
    let prompt = spawned.agent.system_prompt();
    assert!(
        prompt.contains(&child_plan.display().to_string()),
        "the child prompt names its own plan file: {prompt}"
    );
    assert!(
        !prompt.contains(&parent_plan.display().to_string()),
        "the parent's path is replaced: {prompt}"
    );
    // The child's state is observed at spawn.
    assert!(core.plan_state(&spawned.session_id).is_some());
}

/// B1: the section replacement is section-scoped, so a child in a
/// skills-bearing workspace keeps the inherited skills catalog — the
/// catalog is the layer after the plan section, not part of it.
#[tokio::test]
async fn a_spawned_child_keeps_the_inherited_skills_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    // A model-invocable project skill: the parent's prompt carries the
    // catalog as the layer after the plan section.
    std::fs::create_dir_all(cwd.join(".tau").join("skills").join("alpha")).unwrap();
    std::fs::write(
        cwd.join(".tau")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        "---\nname: alpha\ndescription: the alpha skill\n---\nDo alpha.\n",
    )
    .unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = open_ws(&core, &cwd).await;
    let meta = new_root_session(&core, &workspace);
    let live = core.live(&meta.id).unwrap();
    // The precondition: the parent's prompt carries the catalog.
    let parent_prompt = live.agent.system_prompt();
    assert!(
        parent_prompt.contains("<available_skills>"),
        "the parent prompt carries the catalog: {parent_prompt}"
    );
    let sup = live
        .agent
        .subagents()
        .expect("the root session carries a supervisor");

    let spawned = sup
        .spawn("general", "do the thing", None, "call-1")
        .unwrap();
    let prompt = spawned.agent.system_prompt();
    // The child's prompt names its own plan file (the inherited section
    // is replaced, not stacked)...
    assert!(
        prompt.contains(
            &crate::plan::path(&cwd, &spawned.session_id)
                .display()
                .to_string()
        ),
        "the child prompt names its own plan file: {prompt}"
    );
    // ...and the catalog survives the replacement, still the last layer.
    assert!(
        prompt.contains("<available_skills>") && prompt.ends_with("</available_skills>"),
        "the child keeps the inherited skills catalog: {prompt}"
    );
}
