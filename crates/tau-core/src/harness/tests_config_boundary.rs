//! The turn boundary's half of the live reload (spec §12, #337): a session
//! takes the reloaded config at the start of its next turn, never mid-stream.
//! The watcher/reload mechanics — the cache swap, the rejection, the
//! `ProviderChanged` edge — are `tests_config_reload`.

use super::*;

use crate::agent::ContextGate;
use crate::harness::testkit::*;

/// The facts-bearing project layer: the model's declared window and cap, the
/// gate's precondition (#35).
const FACTS: &str = r#"
[providers.dev]
base_url = "http://127.0.0.1:9/v1"

[providers.dev.models.model]
context_window = 32768
max_tokens = 8192

[providers.local]
base_url = "http://127.0.0.1:9/v1"

[providers.local.models.llama3]
context_window = 32768
"#;

/// The two-provider fixture (#337 × #336): `zzz` declares its model's facts,
/// `aaa` — the provider the config's map sorts first — declares none. A route
/// that takes the first provider reads `zzz`'s model in `aaa`'s empty table.
const TWO_PROVIDERS: &str = r#"
[providers.aaa]
base_url = "http://127.0.0.1:1/v1"

[providers.zzz]
base_url = "http://127.0.0.1:2/v1"

[providers.zzz.models.gpt]
context_window = 32768
max_tokens = 8192
"#;
/// A layer's `config.toml`.
fn config_path(layer: &Path) -> PathBuf {
    layer.join(crate::config::CONFIG_FILE)
}

/// The model a session carries, left the way `session_set_model` leaves it: the
/// meta `snapshot()` serves and the agent's own model (what the turn boundary
/// reads) both name it.
fn carry_model(live: &Arc<LiveSession>, model: &str) {
    live.meta.lock().model = Some(model.to_owned());
    live.agent.set_model(model.to_owned());
}

/// The `gate_unavailable` records a session carries — the transition's fact,
/// counted on the store so the test asks the durable question, not the live one.
fn gate_records(cwd: &Path, session: &str) -> usize {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| {
            e.kind == EntryKind::Diagnostic
                && e.payload.get("code").and_then(Value::as_str) == Some("gate_unavailable")
        })
        .count()
}

/// #337 × #336, positively: the turn boundary's re-derivation reads a model's
/// facts from the provider its id names, not from the config's first provider.
/// `zzz/gpt` is declared on the provider that sorts second, so the first
/// provider's table has no facts for it — the pre-#336 route armed no gate.
#[tokio::test]
async fn a_re_derivation_reads_the_provider_the_model_id_names() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    write_config_fixture(&project, TWO_PROVIDERS);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "zzz/gpt");
    core.rederive_turn(&live);

    let turn = live.agent.current_turn();
    assert!(
        turn.context_gate().is_ready(),
        "provider `zzz` declares the window, so naming it arms the gate"
    );
    assert_eq!(
        turn.max_output_tokens,
        Some(8192),
        "the cap is the fact the named provider declares, not the first provider's"
    );
    let address = turn.address.expect("a resolved turn addresses its model");
    assert_eq!(address.id, "zzz/gpt");
    assert_eq!(
        address.key, "gpt",
        "the key is the id with the owning provider's prefix stripped"
    );

    // It is the reloaded file the facts come from: `zzz` lowers its cap (its
    // base_url moves too, so the menu's rows change and the edge lands), and
    // the next boundary takes the new fact from the provider the id names.
    let collected = collect_events(&core);
    write_config_fixture(
        &project,
        &TWO_PROVIDERS
            .replace("http://127.0.0.1:2/v1", "http://127.0.0.1:3/v1")
            .replace("max_tokens = 8192", "max_tokens = 4096"),
    );
    wait_for_provider_changed(&collected, &workspace.id).await;
    core.rederive_turn(&live);
    assert_eq!(
        live.agent.current_turn().max_output_tokens,
        Some(4096),
        "a config edit lands on the model's own provider's new facts"
    );
}

/// #337 × #336, negatively: a model whose prefix names no configured provider
/// is refused at the turn boundary, so the session keeps the facts its provider
/// declared — the config's first provider is never the fallback. The pre-#336
/// route read provider `aaa` (first), found no facts, and dropped the gate.
#[tokio::test]
async fn a_re_derivation_refuses_an_unowned_prefix_over_the_first_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    write_config_fixture(&project, TWO_PROVIDERS);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "zzz/gpt");
    core.rederive_turn(&live);
    assert!(live.agent.current_turn().context_gate().is_ready());
    let session = live.meta.lock().id.clone();

    // `nope` names no configured provider. The refusal keeps this session's
    // derived turn; only the routing is under test, so the model moves here
    // rather than through `session_set_model`, which refuses it earlier.
    carry_model(&live, "nope/gpt");
    core.rederive_turn(&live);

    let turn = live.agent.current_turn();
    assert_eq!(
        turn.max_output_tokens,
        Some(8192),
        "a refusal keeps the facts the route resolved; provider `aaa` declares no cap"
    );
    assert!(
        turn.context_gate().is_ready(),
        "the armed gate survives a refusal: provider `aaa` has no facts, so a turn\
         read from it would be gate-missing"
    );
    assert_eq!(
        gate_records(Path::new(&workspace.cwd), &session),
        0,
        "a refused route records no gate loss"
    );
}

/// A live session takes the edited facts at the next turn boundary: the gate
/// arms from the config the watcher reloaded, mid-session, with no restart
/// (spec §12, #35's route).
#[tokio::test]
async fn a_live_session_re_derives_its_turn_from_the_edited_config() {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    assert_eq!(
        live.agent.current_turn().max_output_tokens,
        Some(u64::from(crate::config::Provider::DEFAULT_MAX_OUTPUT)),
        "the boot config's model carries the declared defaults (#338), so the edit's facts are distinguishable"
    );
    // The cache holds the pre-edit rows (the menu's open read): the watcher's
    // edge compares against them.
    provider_list(&core, &workspace.id);
    let collected = collect_events(&core);
    write_config_fixture(&tmp.path().join(".tau"), FACTS);
    wait_for_provider_changed(&collected, &workspace.id).await;
    core.rederive_turn(&live);
    assert!(
        matches!(
            live.agent.current_turn().context_gate(),
            ContextGate::Ready { .. }
        ),
        "the edited facts arm the gate at the turn boundary"
    );
    assert_eq!(
        live.agent.current_turn().max_output_tokens,
        Some(8192),
        "the model's own cap applies"
    );
    assert_eq!(
        gate_records(Path::new(&workspace.cwd), &live.meta.lock().id),
        0,
        "a gate that armed records nothing"
    );
}

/// The gate's record rides the transition: an edit that strips the facts
/// records `gate_unavailable` once, and a turn whose gate is already missing
/// records nothing more (ADR-0034: the record is the fact, not a per-turn
/// churn).
#[tokio::test]
async fn a_gate_lost_at_the_turn_boundary_records_once() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    write_config_fixture(&project, FACTS);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    let session = live.meta.lock().id.clone();
    // The boundary lands on Ready first (the facts are the file's), so the
    // later strip is a real ready→missing transition.
    core.rederive_turn(&live);
    assert!(live.agent.current_turn().context_gate().is_ready());
    let collected = collect_events(&core);
    std::fs::write(
        config_path(&project),
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\n",
    )
    .unwrap();
    wait_for_provider_changed(&collected, &workspace.id).await;
    core.rederive_turn(&live);
    assert!(
        matches!(
            live.agent.current_turn().context_gate(),
            ContextGate::Missing { .. }
        ),
        "the stripped facts drop the gate"
    );
    assert_eq!(
        gate_records(Path::new(&workspace.cwd), &session),
        1,
        "the transition recorded one fact"
    );
    core.rederive_turn(&live);
    assert_eq!(
        gate_records(Path::new(&workspace.cwd), &session),
        1,
        "a gate that was already missing records nothing new"
    );
}

/// The system layer's config: one provider at the capture server, and the
/// Observer's threshold at `observe`. The model's facts are declared so the
/// context gate stays armed; `buffer_tokens = 0` keeps the background cycle off,
/// so every memory round-trip in these legs belongs to a turn.
fn system_layer(base_url: &str, observe: u64) -> String {
    format!(
        "[providers.dev]\nbase_url = \"{base_url}/v1\"\n\n[providers.dev.models.model]\ncontext_window = 32768\nmax_tokens = 8192\n\n[generation]\ndefault_model = \"dev/model\"\n\n[om]\nom_model = \"\"\nobserve_threshold = {observe}\nreflect_threshold = 100000\nbuffer_increment = 6000\nbuffer_tokens = 0\n"
    )
}

/// The `om` entries a session file holds — the durable witness that a memory
/// pass ran.
fn om_records(cwd: &Path, session: &str) -> usize {
    let mut store = SessionStore::for_workspace(cwd, session);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Om)
        .count()
}

/// A message sent and its turn settled: `ended` counts the `StreamEnd` events
/// already seen, so the caller waits for *its* turn.
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
    // The turn-end memory pass runs before the session goes idle, so an idle
    // session has already written its record.
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

/// #347, positively: the `[om]` thresholds are a live value. A session opened
/// where no pass can run takes a lowered `observe_threshold` at its next turn,
/// and the pass runs at the edited number; a raised number runs none, so the
/// edit is the threshold and not the mere fact of a turn.
#[tokio::test]
async fn an_observe_threshold_edit_runs_the_next_turn_s_pass() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (base, _seen) = crate::harness::tests_om_route::capture_server(
        "<observations>noted</observations>".to_owned(),
    );
    write_config_fixture(system.path(), &system_layer(&base, 100_000));
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
    // ~1800 soft tokens per send: over the edited 1, under the opened 100k.
    let work = "work through this task carefully ".repeat(200);

    send_and_settle(&core, &collected, &session, &work).await;
    assert_eq!(
        om_records(project.path(), &session),
        0,
        "the threshold the session opened with runs no pass"
    );

    write_config_fixture(system.path(), &system_layer(&base, 1));
    core.reload_system_config();
    send_and_settle(&core, &collected, &session, &work).await;
    assert_eq!(
        om_records(project.path(), &session),
        1,
        "the edited threshold runs the session's next pass"
    );

    write_config_fixture(system.path(), &system_layer(&base, 100_000));
    core.reload_system_config();
    send_and_settle(&core, &collected, &session, &work).await;
    assert_eq!(
        om_records(project.path(), &session),
        1,
        "a raised threshold runs no pass; the record the pass wrote stays"
    );
}

/// A session with no memory at all has no thresholds to re-read: the boundary's
/// `[om]` re-derivation creates nothing.
#[tokio::test]
async fn a_session_with_no_memory_re_reads_no_thresholds() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".tau");
    write_config_fixture(&project, FACTS);
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let workspace = open_ws(&core, tmp.path()).await;
    let live = manual_session(
        &core,
        &workspace,
        provider::canned("data: [DONE]\n\n"),
        TurnConfig::default(),
    );
    carry_model(&live, "model");
    core.rederive_turn(&live);
    assert!(
        live.agent.om_state().is_none(),
        "a session launched with no OM gains one from a config edit"
    );
}
