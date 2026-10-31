//! A child session's own memory route: the parent rides `zzz`, the child's agent
//! type names a model of `aaa`, and `om_model` can be a key both providers
//! declare. §6.7 answers that key per session, so the parent's pass is addressed
//! at zzz and the child's at aaa, and a config move reaches the child at its own
//! turn boundary (#348 with #336, #337, #347).

use super::*;

/// The brief a child is spawned with: text no other transcript in the fixture
/// contains, so a request body is attributable to the child.
const BRIEF: &str = "inspect the parser";

/// A `.md` agent type naming `model` as its own (spec §5.5), written before the
/// parent opens: the root assembly discovers types at launch.
fn write_agent_type(project: &Path, name: &str, model: &str) {
    let dir = project.join(".tau").join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: reviews\nmodel: {model}\n---\nYou review.\n"),
    )
    .unwrap();
}

fn spawn(core: &Arc<Core>, parent: &str, agent_type: &str) -> String {
    match core
        .dispatch(Command::SubagentSpawn {
            session: parent.into(),
            agent_type: agent_type.into(),
            brief: BRIEF.into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent.child,
        other => panic!("expected a subagent: {other:?}"),
    }
}

/// An endpoint received a request for `model`.
fn saw_model(seen: &Arc<Mutex<Vec<String>>>, model: &str) -> bool {
    seen.lock()
        .iter()
        .any(|payload| captured_model(payload).as_deref() == Some(model))
}

/// Every `om` record's model field, in append order.
fn om_record_models(cwd: &Path, session: &str) -> Vec<Option<String>> {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Om)
        .map(|e| {
            e.payload
                .get("om_model")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// The newest `om` record's model, waited for past the unstamped ones: the
/// activation record names no model, and it lands before the pass's record.
async fn stamped_om_model(cwd: &Path, session: &str) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(model) = om_record_models(cwd, session).last().cloned().flatten()
            && !model.is_empty()
        {
            return Some(model);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The newest `om` record's model, waited for past `before` records.
async fn om_record_model_after(cwd: &Path, session: &str, before: usize) -> Option<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let models = om_record_models(cwd, session);
        if models.len() > before {
            return models.last().cloned().flatten();
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A route refusal in the session's own record, waited for: the pass that hits
/// it runs at a turn's end, so the diagnostic lands after the turn is idle
/// (#348).
async fn route_refusal(cwd: &Path, session: &str) -> Option<(String, String)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(refusal) = om_failures(cwd, session)
            .into_iter()
            .find(|(stage, _)| stage == "route")
        {
            return Some(refusal);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The parent and the child answer a contested bare key at their own providers.
/// The capture servers are the discriminator: a resolved route is a
/// `Configured { model }`, which names the key, not the endpoint — a pass that
/// travelled to the parent's provider would be invisible in a unit leg.
#[tokio::test]
async fn a_child_and_its_parent_answer_a_contested_om_key_at_their_own_providers() {
    let f = Fixture::new();
    write_agent_type(f.project.path(), "reviewer", "aaa/om-model");
    let (core, parent) = f.start("shared").await;
    let child = spawn(&core, &parent, "reviewer");
    send(&core, &parent).await;

    assert_eq!(
        stamped_om_model(f.project.path(), &child).await.as_deref(),
        Some("shared"),
        "the child's record names the key its pass was addressed with"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !(saw_memory_pass(&f.aaa_seen, "shared", BRIEF)
        && saw_memory_pass(&f.zzz_seen, "shared", SEND_TEXT))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the parent's pass at zzz and the child's at aaa: zzz saw {:?}, aaa saw {:?}",
            models_seen(&f.zzz_seen),
            models_seen(&f.aaa_seen)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw_model(&f.aaa_seen, "om-model"),
        "the child's model calls ran at aaa, so the `shared` request there is \
         the child's memory pass: {:?}",
        models_seen(&f.aaa_seen)
    );
}

/// A child already alive follows a config move: its route is re-derived at its
/// own turn boundary, so the pass after the edit is addressed where the new id
/// names it. The launch-time value is not the claim — a child whose route was
/// frozen at spawn keeps asking for the key it opened on, which is what this
/// leg refuses to accept (#348, #337, #347).
#[tokio::test]
async fn a_config_move_reaches_a_child_that_is_already_alive() {
    let f = Fixture::new();
    write_agent_type(f.project.path(), "reviewer", "aaa/om-model");
    let (core, parent) = f.start("shared").await;
    let child = spawn(&core, &parent, "reviewer");
    send(&core, &parent).await;

    let opened = om_record_models(f.project.path(), &child).len();
    assert_eq!(
        stamped_om_model(f.project.path(), &child).await.as_deref(),
        Some("shared"),
        "the child opens on a pass stamped with the contested key"
    );

    f.write_config("aaa/om-model");
    core.reload_system_config();
    // A pass runs at a turn's end only once the buffer crosses `observe`, so the
    // turn after the edit says enough to be worth reflecting on.
    send_text(&core, &child, &BRIEF.repeat(200)).await;

    assert_eq!(
        om_record_model_after(f.project.path(), &child, opened)
            .await
            .as_deref(),
        Some("om-model"),
        "the pass after the edit is stamped with the model the new id selects"
    );
    assert!(
        saw_memory_pass(&f.aaa_seen, "om-model", BRIEF),
        "the child's pass after the edit asks the provider the new id names for \
         its key: {:?}",
        models_seen(&f.aaa_seen)
    );
}

/// The negative half of that boundary: a config move that kills a live child's
/// route is the child's own durable record, and the pass does not run — the
/// provider it rides is not an address to fall back to (#348, #337).
#[tokio::test]
async fn a_config_move_that_kills_a_childs_route_is_recorded_by_the_child() {
    let f = Fixture::new();
    write_agent_type(f.project.path(), "reviewer", "aaa/om-model");
    let (core, parent) = f.start("aaa/om-model").await;
    let child = spawn(&core, &parent, "reviewer");
    send(&core, &parent).await;
    assert_eq!(
        stamped_om_model(f.project.path(), &child).await.as_deref(),
        Some("om-model"),
        "the child opens on a pass at the provider the id names"
    );

    f.write_config("zzz/ghost");
    core.reload_system_config();
    // A pass runs at a turn's end only once the buffer crosses `observe`, so the
    // turn after the edit says enough to be worth reflecting on.
    send_text(&core, &child, &BRIEF.repeat(200)).await;

    let refusal = route_refusal(f.project.path(), &child)
        .await
        .expect("a child whose route the edit killed records the refusal");
    assert_eq!(refusal.0, "route");
    assert!(
        refusal.1.contains("om.om_model") && refusal.1.contains("zzz/ghost"),
        "the refusal names the config key and the id it cannot address: {}",
        refusal.1
    );
    assert!(
        !saw_memory_pass(&f.aaa_seen, "ghost", BRIEF)
            && !saw_memory_pass(&f.zzz_seen, "ghost", BRIEF),
        "a refusal asks no provider for a model none declares: aaa={:?} zzz={:?}",
        models_seen(&f.aaa_seen),
        models_seen(&f.zzz_seen)
    );
    assert_eq!(
        om_record_models(f.project.path(), &child)
            .last()
            .cloned()
            .flatten(),
        Some("om-model".to_owned()),
        "the refusal runs no pass: the newest record is still the pre-edit one"
    );
}
