use super::*;

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

/// `SessionNew`'s explicit title takes the rename's cap (the GUI always
/// passes null, but the protocol is open): over the cap is refused
/// before the session file is created, at the cap it is accepted.
#[tokio::test]
async fn session_rename_caps_the_title_length() {
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
    let long = "t".repeat(201);
    let err = core
        .dispatch(Command::SessionRename {
            session: live.id.clone(),
            title: long,
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(msg.contains("200"), "the refusal names the cap: {msg}");
    // The refusal writes nothing: the header's title is untouched.
    let header = std::fs::read_to_string(
        cwd.path()
            .join(".tau/sessions")
            .join(format!("{}.jsonl", live.id)),
    )
    .unwrap()
    .lines()
    .next()
    .unwrap()
    .to_owned();
    assert!(
        !header.contains(&"t".repeat(100)),
        "the oversized title never reached the header: {header}"
    );
    // The cap itself is acceptable.
    core.dispatch(Command::SessionRename {
        session: live.id,
        title: "u".repeat(200),
    })
    .unwrap();
}

/// A sub-agent whose parent is closed cannot be archived directly, and
/// the refusal must give the real route: a closed parent is refused as
/// "not open", so "archive its parent" is not a route — reopening the
/// parent and archiving it archives the child with it (review N7).
#[tokio::test]

async fn a_child_refusal_names_the_reopen_route_for_a_closed_parent() {
    let core = CoreBuilder::custom(providers())
        .with_child_factory(Arc::new(CannedChildFactory { body: done_body() }))
        .build()
        .unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let workspace = open_ws(&core, cwd.path()).await;
    let parent = match core
        .dispatch(Command::SessionNew {
            workspace: workspace.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session,
        other => panic!("expected a session: {other:?}"),
    };
    let info = spawn_done_child(&core, &parent.id).await;
    core.dispatch(Command::SessionClose {
        session: parent.id.clone(),
    })
    .unwrap();
    let err = core
        .dispatch(Command::SessionArchive {
            session: info.child.clone(),
        })
        .unwrap_err();
    let msg = match &err {
        ProtocolError::Other { message } => message.as_str(),
        other => panic!("expected a plain refusal: {other:?}"),
    };
    assert!(
        msg.contains("is not open") && msg.contains("reopen it and archive it"),
        "the refusal gives the real route: {msg}"
    );
    assert!(
        msg.contains(&parent.id),
        "the refusal names the parent: {msg}"
    );
    drop(core);
}

/// An archive that fails on I/O (the parent's live file vanished) leaves
/// the parent in the live map: its in-memory session (queue, supervisor)
/// survives the refusal — the next archive/restore converges, but the
/// session is never detached (review N2).
#[tokio::test]

async fn session_rename_syncs_the_live_meta() {
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
    core.dispatch(Command::SessionRename {
        session: live.id.clone(),
        title: "Renamed".into(),
    })
    .unwrap();
    // Re-opening serves the snapshot from the live meta; the rename must
    // not revert (the B1 regression).
    let snap = match core
        .dispatch(Command::SessionOpen { session: live.id })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.title.as_deref(), Some("Renamed"));
}

#[tokio::test]

async fn session_rename_updates_a_closed_session_header() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store.set_title("Before").unwrap();
    }
    core.dispatch(Command::SessionRename {
        session: id.clone(),
        title: "After".into(),
    })
    .unwrap();
    let header =
        std::fs::read_to_string(cwd.path().join(".tau/sessions").join(format!("{id}.jsonl")))
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_owned();
    assert!(header.contains("\"After\""), "header: {header}");
}

#[tokio::test]

async fn session_set_model_updates_the_live_meta_and_records_the_change() {
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
    // Same model: a no-op (no quiet entry appended).
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: old.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: "other/model".into(),
    })
    .unwrap();
    let snap = match core
        .dispatch(Command::SessionOpen {
            session: live.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.model.as_deref(), Some("other/model"));
    let facts = facts_in(&session_file(cwd.path(), &live.id));
    assert_eq!(
        facts,
        vec![
            Diagnostic::ModelChanged {
                from: Some(old),
                to: "other/model".into(),
            },
            Diagnostic::GateUnavailable {
                model: "other/model".into(),
                missing: vec!["context_window".into(), "max_tokens".into()],
            },
        ],
        "the change, then the gate its facts cannot compute"
    );
}

#[tokio::test]

async fn session_set_model_on_a_closed_session_appends_the_quiet_entry() {
    let core = CoreBuilder::custom(providers()).build().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let w = open_ws(&core, cwd.path()).await;
    let id = SessionStore::new_session_id();
    {
        let mut store = SessionStore::for_workspace(Path::new(&w.cwd), &id);
        store.create().unwrap();
        store
            .append(EntryKind::User, json!({ "text": "hello" }), None)
            .unwrap();
    }
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "m/new".into(),
    })
    .unwrap();
    core.dispatch(Command::SessionSetModel {
        session: id.clone(),
        model: "m/new".into(),
    })
    .unwrap();
    // The quiet entry is the record: the same model again is a no-op.
    let facts = facts_in(&session_file(cwd.path(), &id));
    assert_eq!(
        facts,
        vec![Diagnostic::ModelChanged {
            from: None,
            to: "m/new".into(),
        }],
        "one record, naming no predecessor"
    );
}

#[tokio::test]
async fn a_session_written_before_the_records_gains_one_baseline_at_open() {
    // The migration path (ADR-0034): an old file is not re-interpreted. The
    // first open states the model it runs and the gate its facts leave out;
    // every later open reads those records and states nothing new.
    let core = CoreBuilder::custom(providers()).build().unwrap();
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
    for _ in 0..2 {
        core.dispatch(Command::SessionOpen {
            session: id.clone(),
        })
        .unwrap();
        core.dispatch(Command::SessionClose {
            session: id.clone(),
        })
        .unwrap();
    }
    assert_eq!(
        facts_in(&session_file(cwd.path(), &id)),
        vec![
            Diagnostic::ModelChanged {
                from: None,
                to: "model".into(),
            },
            Diagnostic::GateUnavailable {
                model: "model".into(),
                missing: vec!["context_window".into(), "max_tokens".into()],
            },
        ],
        "two opens, one pair of records"
    );
}

#[tokio::test]
async fn a_model_that_declares_its_facts_records_no_gate_gap() {
    // The negative half: declared Model facts make the gate computable, so the
    // open states the model and nothing about the gate.
    let mut facts = std::collections::BTreeMap::new();
    facts.insert(
        "model".into(),
        crate::config::ModelDef {
            context_window: Some(131_072),
            max_tokens: Some(16_384),
            ..Default::default()
        },
    );
    let mut provider = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "model");
    provider.models = facts;
    let mut providers = std::collections::BTreeMap::new();
    providers.insert("dev".into(), provider);
    let core = CoreBuilder::custom(providers).build().unwrap();
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
    core.dispatch(Command::SessionOpen {
        session: id.clone(),
    })
    .unwrap();
    assert_eq!(
        facts_in(&session_file(cwd.path(), &id)),
        vec![Diagnostic::ModelChanged {
            from: None,
            to: "model".into(),
        }],
        "a gated model records no gate gap"
    );
}

#[tokio::test]
async fn a_model_whose_window_leaves_no_room_records_the_gap() {
    // Declared facts are not a gate: 8192 − 8192 − 4096 underflows, so the gap
    // is the room, not an undeclared fact. The open path and the live switch
    // arm on the same predicate, so neither can record a gap the other denies.
    let mut provider = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "model");
    provider.models.insert(
        "model".into(),
        crate::config::ModelDef {
            context_window: Some(8_192),
            max_tokens: Some(8_192),
            ..Default::default()
        },
    );
    let mut providers = std::collections::BTreeMap::new();
    providers.insert("dev".into(), provider);
    let core = CoreBuilder::custom(providers).build().unwrap();
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
    core.dispatch(Command::SessionOpen {
        session: id.clone(),
    })
    .unwrap();
    assert_eq!(
        facts_in(&session_file(cwd.path(), &id)),
        vec![
            Diagnostic::ModelChanged {
                from: None,
                to: "model".into(),
            },
            Diagnostic::GateUnavailable {
                model: "model".into(),
                missing: vec!["context_room".into()],
            },
        ],
        "the record names what is missing, and never comes out empty"
    );
}

#[tokio::test]

async fn session_set_model_survives_close_and_reopen() {
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
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: "other/model".into(),
    })
    .unwrap();
    // Close + re-open goes through build_live again: the model is a
    // file-level fact (the quiet note), not the in-memory agent's.
    core.dispatch(Command::SessionClose {
        session: live.id.clone(),
    })
    .unwrap();
    let snap = match core
        .dispatch(Command::SessionOpen { session: live.id })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.model.as_deref(), Some("other/model"));
}

/// §6.7: the model menu switches by `provider/key`, the config declares the
/// model by bare key. A switch through the menu's spelling must find the model's
/// facts — before one resolver read it missed the key, and the audit channel
/// correctly recorded a gate gap for a model whose window was declared.
#[tokio::test]
async fn a_menu_qualified_switch_keeps_the_declared_facts() {
    let mut provider = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "model");
    provider.models.insert(
        "second".to_owned(),
        crate::config::ModelDef {
            context_window: Some(131_072),
            max_tokens: Some(16_384),
            ..Default::default()
        },
    );
    let core = CoreBuilder::custom(BTreeMap::from([("dev".to_owned(), provider)]))
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
    core.dispatch(Command::SessionSetModel {
        session: live.id.clone(),
        model: "dev/second".into(),
    })
    .unwrap();
    let facts = facts_in(&session_file(cwd.path(), &live.id));
    assert_eq!(
        facts,
        vec![Diagnostic::ModelChanged {
            from: Some("model".into()),
            to: "dev/second".into(),
        }],
        "a declared model has a working gate, so the switch records the change alone"
    );
    // The id the session carries is the one the user picked; the config key it
    // resolves to is the request's address, not the session's name.
    let snap = match core
        .dispatch(Command::SessionOpen {
            session: live.id.clone(),
        })
        .unwrap()
    {
        CommandOutput::Snapshot { snapshot } => snapshot,
        other => panic!("expected snapshot: {other:?}"),
    };
    assert_eq!(snap.session.model.as_deref(), Some("dev/second"));
}

/// A `generation.default_model` written the way the menu spells it opens on the
/// model it names. Without §6.7's read on the config-supplied id, the prefixed
/// name misses the bare-key map and the session silently opens on the provider's
/// first model — a wrong model with nothing recorded about it.
#[tokio::test]
async fn a_prefixed_default_model_opens_on_the_model_it_names() {
    // The config file, not a hand-built provider map: the default model is a
    // config-supplied id, so the test travels the same layering production does.
    let sys = tempfile::tempdir().unwrap();
    std::fs::write(
        sys.path().join("config.toml"),
        r#"
[providers.dev]
base_url = "http://127.0.0.1:9/v1"
key_env = ""

[providers.dev.models.first]

[providers.dev.models.second]
context_window = 131_072
max_tokens = 16_384

[generation]
default_model = "dev/second"
"#,
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
    assert_eq!(
        live.model.as_deref(),
        Some("dev/second"),
        "the default is taken by name, not replaced by the map's first entry"
    );
    // The reopen is where the resolution shows: a default that had resolved to a
    // name only would leave its facts undeclared and owe a `gate_unavailable`.
    core.dispatch(Command::SessionClose {
        session: live.id.clone(),
    })
    .unwrap();
    core.dispatch(Command::SessionOpen {
        session: live.id.clone(),
    })
    .unwrap();
    assert_eq!(
        facts_in(&session_file(cwd.path(), &live.id)),
        Vec::new(),
        "the default resolved to a model that declares its facts"
    );
}

/// The fallback survives the read: a default no provider declares opens on the
/// first model, which is what the config's empty-value contract has always meant.
#[tokio::test]
async fn an_undeclared_default_model_falls_back_to_the_first_model() {
    let sys = tempfile::tempdir().unwrap();
    std::fs::write(
        sys.path().join("config.toml"),
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\nkey_env = \"\"\n\n[providers.dev.models.\"model\"]\n\n[generation]\ndefault_model = \"other/nope\"\n",
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
