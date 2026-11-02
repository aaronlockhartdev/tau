//! The switch leg of the model surface: the fall-back when a config default names
//! no provider, the endpoint that follows the model a switch selects, and the
//! prefix that resolves a shared key's facts.

use super::*;
use serde_json::json;

use crate::harness::testkit::*;

/// A session file's records, in append order.
fn facts_in(file: &str) -> Vec<Diagnostic> {
    file.lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            (v.get("type")?.as_str()? == "diagnostic")
                .then(|| v.get("payload").and_then(Diagnostic::from_value))
                .flatten()
        })
        .collect()
}

fn session_file(cwd: &Path, id: &str) -> String {
    std::fs::read_to_string(cwd.join(".tau/sessions").join(format!("{id}.jsonl"))).unwrap()
}

/// The fallback survives the read: a default no provider declares opens on the
/// first model, which is what the config's empty-value contract has always meant.
#[tokio::test]
async fn an_undeclared_default_model_falls_back_to_the_first_model() {
    let sys = tempfile::tempdir().unwrap();
    std::fs::write(
        sys.path().join("config.toml"),
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\nkey_env = \"\"\n\n[providers.dev.models.\"model\"]\ncontext_window = 32768\n\n[generation]\ndefault_model = \"other/nope\"\n",
    )
    .unwrap();
    let core = CoreBuilder::default_system()
        .unwrap()
        .with_system_dir(sys.path().to_path_buf())
        .with_home(sys.path().to_path_buf())
        .build()
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    // `other` is not a configured provider, so the id is not stripped and matches
    // nothing: the fallback, not a guess at a same-named key.
    assert_eq!(live.model.as_deref(), Some("model"));
}

/// One send from idle, and the bounded wait for its turn to settle: the canned
/// scripts stream at 25 ms per event, so a turn takes about a second.
async fn send_and_settle(core: &Arc<Core>, id: &str, text: &str) {
    core.dispatch(Command::MessageSend {
        session: id.into(),
        text: text.into(),
        lane: MessageLane::Steering,
    })
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let running = core
            .sessions
            .lock()
            .get(id)
            .is_some_and(|l| l.turn.load(Ordering::SeqCst));
        if !running {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn never settled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The session's assistant entries as (text, reasoning), in record order.
fn assistant_turns(cwd: &Path, id: &str) -> Vec<(String, String)> {
    let mut store = SessionStore::for_workspace(cwd, id);
    store.open().unwrap();
    store
        .entries_range(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|e| e.kind == EntryKind::Assistant)
        .map(|e| {
            (
                e.payload["text"].as_str().unwrap_or("").to_owned(),
                e.payload["reasoning"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

fn session_model(core: &Arc<Core>, id: &str) -> Option<String> {
    match core
        .dispatch(Command::SessionOpen { session: id.into() })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot.session.model,
        other => panic!("expected snapshot: {other:?}"),
    }
}

/// #336's acceptance bar, the half the call count cannot show: the endpoint
/// follows the model. `live.provider.calls` counts calls on the forwarding
/// wrapper, so a send always makes exactly one no matter which provider answers.
/// The two canned scripts discriminate the endpoint — only `canned://reasoning`
/// emits reasoning — so the assistant entry names the provider that answered.
#[tokio::test]
async fn the_endpoint_follows_the_model_a_switch_selects() {
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        "aaa".to_owned(),
        crate::config::Provider::with_model("canned://text", "text-model"),
    );
    providers.insert(
        "zzz".to_owned(),
        crate::config::Provider::with_model("canned://reasoning", "reasoning-model"),
    );
    let core = CoreBuilder::custom(providers).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    let id = live.id.clone();

    // The session opens on the config default — the first provider's model —
    // and its script answers with text and no reasoning.
    send_and_settle(&core, &id, "first").await;
    let turns = assistant_turns(cwd.path(), &id);
    assert_eq!(turns.len(), 1, "one send, one assistant entry");
    assert!(
        turns[0].0.starts_with("word 0"),
        "the first provider's text script answered: {:?}",
        turns[0].0
    );
    assert_eq!(
        turns[0].1, "",
        "the text script emits no reasoning, so its absence identifies the endpoint"
    );

    // The menu's spelling of the second provider's model: the next call answers
    // to that provider, whose script is the only source of reasoning deltas.
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "zzz/reasoning-model".into(),
    })
    .unwrap();
    assert_eq!(
        session_model(&core, &id).as_deref(),
        Some("zzz/reasoning-model"),
        "the meta and the chip carry the model that is in use"
    );
    send_and_settle(&core, &id, "second").await;
    let turns = assistant_turns(cwd.path(), &id);
    assert_eq!(turns.len(), 2);
    assert!(
        turns[1].1.starts_with("thought 0"),
        "the second provider answered: {:?}",
        turns[1]
    );

    // "Preserved" means the endpoint follows the record, not just the live
    // switch: after close and reopen the model the file names is routed again.
    core.dispatch(Command::SessionClose {
        session: id.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionOpen {
        session: id.clone(),
    })
    .unwrap();
    assert_eq!(
        session_model(&core, &id).as_deref(),
        Some("zzz/reasoning-model"),
        "the reopen carries the recorded model"
    );
    send_and_settle(&core, &id, "third").await;
    let turns = assistant_turns(cwd.path(), &id);
    assert_eq!(turns.len(), 3);
    assert!(
        turns[2].1.starts_with("thought 0"),
        "the reopen addressed the model's own provider: {:?}",
        turns[2]
    );
}

/// #336, the facts half: a model's `context_window` / `max_tokens` resolve from
/// the provider its id routes to. The same bare key is declared on both
/// providers with opposite gate facts, so a prefix is the only thing that can
/// flip the recorded gap — and a bare key two providers declare stays on the
/// provider the session is already addressed at (the documented tie rule, not
/// `BTreeMap` order).
#[tokio::test]
async fn a_shared_model_key_resolves_its_facts_from_the_prefix() {
    let mut first = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "shared");
    first.models.insert(
        "shared".to_owned(),
        crate::config::ModelDef {
            context_window: 8_192,
            max_tokens: Some(8_192),
            ..Default::default()
        },
    );
    let mut second = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "shared");
    second.models.insert(
        "shared".to_owned(),
        crate::config::ModelDef {
            context_window: 131_072,
            max_tokens: Some(16_384),
            ..Default::default()
        },
    );
    let core = CoreBuilder::custom(std::collections::BTreeMap::from([
        ("aaa".to_owned(), first),
        ("zzz".to_owned(), second),
    ]))
    .build()
    .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store
            .append(EntryKind::User, json!({ "text": "a turn" }), None)
            .unwrap();
    }
    // The open is on the bare key, which two providers declare, so it stays with
    // the provider it starts from — the first — whose window leaves no room.
    core.dispatch(Command::SessionOpen {
        session: id.clone(),
    })
    .unwrap();
    for model in ["zzz/shared", "shared", "aaa/shared"] {
        core.dispatch(Command::SessionSetModel {
            session: id.clone(),
            model: model.into(),
        })
        .unwrap();
    }
    assert_eq!(
        facts_in(&session_file(cwd.path(), &id)),
        vec![
            Diagnostic::ModelChanged {
                from: None,
                to: "shared".into(),
            },
            Diagnostic::GateUnavailable {
                model: "shared".into(),
                missing: vec!["context_room".into()],
            },
            // The second provider's facts gate the model: the switch records the
            // change and says nothing about the gate.
            Diagnostic::ModelChanged {
                from: Some("shared".into()),
                to: "zzz/shared".into(),
            },
            // The bare key, declared by both, stays on the provider the session
            // is on, so the clean gate stands.
            Diagnostic::ModelChanged {
                from: Some("zzz/shared".into()),
                to: "shared".into(),
            },
            // The first provider's prefix names the owner again and the gap
            // returns: the prefix, not the map's order, chose the facts.
            Diagnostic::ModelChanged {
                from: Some("shared".into()),
                to: "aaa/shared".into(),
            },
            Diagnostic::GateUnavailable {
                model: "aaa/shared".into(),
                missing: vec!["context_room".into()],
            },
        ],
        "the gate tracks the provider the id's prefix names"
    );
}

/// #336's negative: an id whose prefix names no configured provider addresses no
/// endpoint, so the switch is refused. At HEAD it travelled to the provider the
/// config's map sorts first, where the model does not exist — the report's burst
/// of 400s — and the chip showed it as live.
#[tokio::test]
async fn a_model_whose_prefix_names_no_provider_is_refused() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let live = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected session: {other:?}"),
    };
    let old = live.model.clone().expect("a fresh session has a model");
    let err = core
        .dispatch(Command::SessionSetModel {
            session: live.id.clone(),
            model: "nosuch/x".into(),
        })
        .expect_err("no provider is named `nosuch`");
    assert!(
        matches!(&err, ProtocolError::Other { message }
            if message.contains("no configured provider") && message.contains("nosuch")),
        "the refusal names the prefix it cannot route: {err}"
    );
    // The refusal moves nothing: the model the session carries, and the record,
    // are what they were.
    assert_eq!(
        session_model(&core, &live.id).as_deref(),
        Some(old.as_str())
    );
    assert_eq!(
        facts_in(&session_file(cwd.path(), &live.id)),
        Vec::<Diagnostic>::new(),
        "a refused switch leaves no `model_changed`, and no gate note"
    );
}
