//! The generation profile on the live path (#407): `[om.generation]` rides
//! the Observer's wire body — sampling, the reasoning policy, the
//! `extra_body` passthrough — and a file edit moves the profile at the
//! session's next turn boundary, the same rule #347 set for `[om]`.

use super::*;

use crate::harness::testkit::*;

/// The system layer: the session's model on `dev` at the capture server,
/// `[thinking] level = "medium"` globally, and the OM role pinned by
/// `[om.generation]` — `temperature`, `thinking = "off"`, and
/// `extra_body.thinkingBudget` when `budget` names one. The 6000-token
/// interval keeps the background cycles quiet below the third send.
fn system_layer(base_url: &str, observe: u64, temperature: f64, budget: Option<u64>) -> String {
    let extra = budget.map_or(String::new(), |b| {
        format!("\n[om.generation.extra_body]\nthinkingBudget = {b}\n")
    });
    format!(
        "[providers.dev]\nbase_url = \"{base_url}/v1\"\n\n\
         [providers.dev.models.model]\ncontext_window = 32768\nmax_tokens = 8192\n\n\
         [providers.dev.models.model.generation]\nmax_tokens = 4096\n\n\
         [providers.dev.models.model.generation.extra_body]\nvendorKey = 7\n\n\
         [generation]\ndefault_model = \"dev/model\"\n\n\
         [thinking]\nlevel = \"medium\"\n\n\
         [om]\nmodel = \"dev/model\"\nobserve_threshold = {observe}\n\
         reflect_threshold = 100000\nbuffer_increment = 6000\n\n\
         [om.generation]\ntemperature = {temperature}\nthinking = \"off\"\n{extra}"
    )
}

/// The bodies the capture server saw, parsed, that are Observer calls:
/// their `instructions` are the observer system prompt verbatim.
fn observer_bodies(seen: &Arc<Mutex<Vec<String>>>) -> Vec<serde_json::Value> {
    let prompt = crate::om::observer_system_prompt();
    seen.lock()
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .filter(|v: &serde_json::Value| v["instructions"] == prompt.as_str())
        .collect()
}

fn main_bodies(seen: &Arc<Mutex<Vec<String>>>) -> Vec<serde_json::Value> {
    let prompt = crate::om::observer_system_prompt();
    seen.lock()
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .filter(|v: &serde_json::Value| v["instructions"] != prompt.as_str())
        .collect()
}

/// A message sent and its turn settled (the boundary's idle witness, #347).
async fn send_and_settle(
    core: &Arc<Core>,
    collected: &Arc<Mutex<Vec<Event>>>,
    session: &str,
    text: &str,
) {
    let ended = stream_ends(collected);
    core.dispatch(Command::MessageSend {
        session: session.to_owned(),
        text: text.to_owned(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if stream_ends(collected) > ended {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn never settled for {session}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let live = core.live(session).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while live.turn.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session never went idle after {session}'s turn"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn stream_ends(collected: &Arc<Mutex<Vec<Event>>>) -> usize {
    collected
        .lock()
        .iter()
        .filter(|e| matches!(e, Event::StreamEnd { .. }))
        .count()
}

/// #407 end to end: the `[om.generation]` profile rides the Observer's wire
/// body (sampling, `extra_body` scalars, and the reasoning policy `off` —
/// the global `[thinking]` level reaches the main call, not the Observer);
/// editing the file moves the profile at the next turn, and removing the
/// `extra_body` table removes the scalars from the next call.
#[tokio::test]
async fn the_generation_profile_rides_the_observer_call_and_moves_live() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (base, seen) = crate::harness::tests_om_route::capture_server(
        "<observations>noted</observations>".to_owned(),
    );
    write_config_fixture(system.path(), &system_layer(&base, 100_000, 0.3, Some(215)));
    let core = CoreBuilder::default_system()
        .unwrap()
        .with_system_dir(system.path().to_path_buf())
        .with_home(system.path().to_path_buf())
        .build()
        .unwrap();
    let workspace = open_ws(&core, project.path()).await;
    let session = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session.id,
        other => panic!("expected a session: {other:?}"),
    };
    let collected = collect_events(&core);
    // ~825 soft tokens per send: the second settle's window (~1,650) crosses
    // the edited 1,000 — its slice plus the drain consume the window, so the
    // third send (~1,070) crosses activation on its own. Every volume stays
    // under the 3,000 trickle floor: no background cycle runs, and every
    // Observer call in these phases belongs to a settle.
    let work = "work through this task carefully ".repeat(100);
    let long = "work through this task carefully ".repeat(130);

    // The negative: below the threshold, no Observer call exists to check.
    send_and_settle(&core, &collected, &session, &work).await;
    assert!(
        observer_bodies(&seen).is_empty(),
        "the opened threshold runs no pass"
    );
    // The main call carries the global reasoning level — the wire shape the
    // OM assertion below contrasts against.
    let main = main_bodies(&seen)
        .pop()
        .expect("the main call was captured");
    assert_eq!(
        main["reasoning"]["effort"].as_str(),
        Some("medium"),
        "the session's model runs the global [thinking] level"
    );
    assert_eq!(
        main["vendorKey"], 7,
        "the model profile's extra_body rides the main call verbatim"
    );

    write_config_fixture(system.path(), &system_layer(&base, 1_000, 0.3, Some(215)));
    reload_system_layer(&core);
    send_and_settle(&core, &collected, &session, &work).await;
    let calls = observer_bodies(&seen);
    assert!(!calls.is_empty(), "the edited threshold runs the pass");
    for call in &calls {
        assert_eq!(
            call["temperature"], 0.3,
            "the role profile's sampling rides the Observer call"
        );
        assert_eq!(
            call["thinkingBudget"], 215,
            "the extra_body scalar flattens into the body verbatim"
        );
        assert_eq!(
            call["max_output_tokens"], 4096,
            "the OM model's profile reaches the role through the merge"
        );
        assert!(
            call.get("reasoning").is_none(),
            "the profile's thinking = off silences the Observer even \
             while the global [thinking] level runs the main call"
        );
    }

    write_config_fixture(system.path(), &system_layer(&base, 1_000, 0.1, None));
    reload_system_layer(&core);
    let calls_before = observer_bodies(&seen).len();
    send_and_settle(&core, &collected, &session, &long).await;
    let calls = observer_bodies(&seen);
    assert!(
        calls.len() > calls_before,
        "the moved profile runs on the pass the same edit arms"
    );
    for call in &calls[calls_before..] {
        assert_eq!(call["temperature"], 0.1, "the moved sampling rides");
        assert!(
            call.get("thinkingBudget").is_none(),
            "dropping the extra_body table drops the scalars"
        );
    }
}
