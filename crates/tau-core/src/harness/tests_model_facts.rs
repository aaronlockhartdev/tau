//! The gate-facts leg of the model surface: what an open or a switch records
//! about a model's `context_window` / `max_tokens`, and the §6.7 read that takes
//! a model id — the menu's `provider/key`, a gateway's own `vendor/name` — to the
//! provider declaring its facts.

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

#[tokio::test]
async fn a_model_that_declares_its_facts_records_no_gate_gap() {
    // The negative half: declared Model facts make the gate computable, so the
    // open states the model and nothing about the gate.
    let mut facts = std::collections::BTreeMap::new();
    facts.insert(
        "model".into(),
        crate::config::ModelDef {
            context_window: 131_072,
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
            context_window: 8_192,
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
        model: "undeclared".into(),
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
    assert_eq!(snap.session.model.as_deref(), Some("undeclared"));
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
            context_window: 131_072,
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
context_window = 8192

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
    // The reopen is where the resolution shows, and only once the session has a
    // branch to project: an empty session records nothing whatever the gate says.
    // A default that had resolved to a name alone would leave its facts undeclared
    // and owe a `gate_unavailable` beside the model it states.
    {
        let mut store = SessionStore::for_workspace(cwd.path(), &live.id);
        store
            .append(EntryKind::User, json!({ "text": "a turn" }), None)
            .unwrap();
    }
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
        vec![Diagnostic::ModelChanged {
            from: None,
            to: "dev/second".into(),
        }],
        "the default resolved to a model that declares its facts"
    );
}

/// #358: a gateway's config declares the vendor's own ids — `openrouter`'s catalog
/// spells all of them `vendor/name` — and a user sets the default to what the
/// vendor prints. §6.7 takes that id to the provider declaring it. Before this,
/// §6.7 read `openai` as a provider name, found none, and the session opened on
/// the map's first model with nothing recorded about the substitution.
#[tokio::test]
async fn a_gateways_slashed_default_model_opens_on_the_model_it_names() {
    let sys = tempfile::tempdir().unwrap();
    // `alpha` sorts first, so landing on it is the fall-back this leg refuses.
    std::fs::write(
        sys.path().join("config.toml"),
        "[providers.alpha]\nbase_url = \"http://127.0.0.1:9/a/v1\"\nkey_env = \"\"\n\n[providers.alpha.models.first]\ncontext_window = 8192\n\n[providers.openrouter]\nbase_url = \"http://127.0.0.1:9/o/v1\"\nkey_env = \"\"\n\n[providers.openrouter.models.\"openai/gpt-5\"]\ncontext_window = 200_000\nmax_tokens = 8192\n\n[generation]\ndefault_model = \"openai/gpt-5\"\n",
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
        Some("openai/gpt-5"),
        "the vendor's id is taken at its declarer, not replaced by `alpha`'s first model"
    );
    // A record to project: an empty session states no facts whatever the gate says.
    {
        let mut store = SessionStore::for_workspace(cwd.path(), &live.id);
        store
            .append(EntryKind::User, json!({ "text": "a turn" }), None)
            .unwrap();
    }
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
        vec![Diagnostic::ModelChanged {
            from: None,
            to: "openai/gpt-5".into(),
        }],
        "the route bound openrouter's declared facts, so the gate records no gap"
    );
}
