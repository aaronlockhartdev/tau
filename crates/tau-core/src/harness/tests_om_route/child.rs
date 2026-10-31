//! A child session's own memory route: the parent rides `zzz`, the child's agent
//! type names a model of `aaa`, and `om_model` can be a key both providers
//! declare. §6.7 answers that key per session, so the parent's pass is addressed
//! at zzz and the child's at aaa; a config move reaches the child at its own
//! turn boundary, and a move that kills its route is the child's own refusal
//! (#348 with #336, #337, #347).

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

/// What an endpoint was asked for, model by model — the readable form for an
/// assertion message, where a payload dump would bury the answer.
fn models_seen(seen: &Arc<Mutex<Vec<String>>>) -> Vec<Option<String>> {
    seen.lock()
        .iter()
        .map(|payload| captured_model(payload))
        .collect()
}

/// An endpoint ran a memory pass for `model` over a transcript containing `text`:
/// the model names the address, the transcript says which session ran there.
fn saw_memory_pass(seen: &Arc<Mutex<Vec<String>>>, model: &str, text: &str) -> bool {
    seen.lock().iter().any(|payload| {
        captured_model(payload).as_deref() == Some(model)
            && payload.contains(MEMORY_INSTRUCTIONS)
            && payload.contains(text)
    })
}

/// How many memory passes an endpoint has been asked to run. Every memory
/// round-trip — Observer or Reflector — carries the memory prompt, and the mark
/// is on the wire before any answer is parsed, so a count is a witness no failed
/// parse can hide.
fn memory_passes(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock()
        .iter()
        .filter(|payload| payload.contains(MEMORY_INSTRUCTIONS))
        .count()
}

/// A route refusal in the session's own record, waited for: the agent task
/// appends the diagnostic and this reader opens its own handle to the file, so
/// it polls for the append rather than assuming it landed (#348).
async fn route_refusal(cwd: &Path, session: &str) -> Option<(String, String)> {
    let deadline = tokio::time::Instant::now() + SETTLE;
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
    let deadline = tokio::time::Instant::now() + SETTLE;
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

    assert_eq!(
        stamped_om_model(f.project.path(), &child).await.as_deref(),
        Some("shared"),
        "the child opens on a pass stamped with the contested key"
    );
    let opened = om_record_models(f.project.path(), &child).len();

    f.write_config("aaa/om-model");
    core.reload_system_config();
    // The turn after the edit repeats the brief: a pass reads the transcript
    // after its cursor, so the body it sends is this turn's text and a request
    // naming BRIEF at an endpoint is attributable to the child.
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
    // The witness that a refusal runs no pass: every memory round-trip carries the
    // memory prompt, and the capture server logs a request before it answers, so a
    // pass shows up whatever model it rides. A pass's stamp is no such witness — it
    // reaches a record only with a chunk delta, so a refused pass can leave the
    // records untouched. `buffer_tokens` is 0 in this fixture, so no detached
    // background cycle can add a request outside the turn's settle.
    let passes = memory_passes(&f.aaa_seen) + memory_passes(&f.zzz_seen);

    f.write_config("zzz/ghost");
    core.reload_system_config();
    // As above: the repeated brief makes any request that does reach an
    // endpoint attributable to the child, which is what a refusal must prevent.
    send_text(&core, &child, &BRIEF.repeat(200)).await;

    let refusal = route_refusal(f.project.path(), &child)
        .await
        .expect("a child whose route the edit killed records the refusal");
    assert!(
        refusal.1.contains("om.om_model") && refusal.1.contains("zzz/ghost"),
        "the refusal names the config key and the id it cannot address: {}",
        refusal.1
    );
    // A second turn: a refusal that also asks someone does it at a turn's end, so
    // the session has to reach a turn's end before the endpoints are read.
    send_text(&core, &child, &BRIEF.repeat(200)).await;
    settle(&core, &child).await;
    assert_eq!(
        memory_passes(&f.aaa_seen) + memory_passes(&f.zzz_seen),
        passes,
        "the refusal runs no pass: no endpoint is asked to run a memory pass after \
         the edit: aaa={:?} zzz={:?}",
        models_seen(&f.aaa_seen),
        models_seen(&f.zzz_seen)
    );
}
