//! #336's child leg: a subagent's calls answer to the provider its own model's
//! id names. Both legs run the default `ForwardingChildFactory` (no
//! `with_child_factory`) — the production route — with the two canned scripts as
//! the endpoint discriminator: only `canned://reasoning` emits reasoning, so the
//! child's assistant entry says which provider answered.

use super::*;

use crate::harness::testkit::*;

/// A config whose providers sort apart: `aaa` is the map's first entry and the
/// parent opens on `zzz`, so "the first provider" and "the parent's provider"
/// are never the same answer — a route that travels to the map's first entry is
/// visible, not silent.
const CONFIG: &str = r#"
[providers.aaa]
base_url = "canned://text"

[providers.aaa.models.text-model]
context_window = 32768

[providers.zzz]
base_url = "canned://reasoning"

[providers.zzz.models.reasoning-model]
context_window = 32768
max_tokens = 8192

# The key the unroutable id `nosuch/model` resolves to inside the parent's
# provider: #338 requires a declared window for a gated turn.
[providers.zzz.models.model]
context_window = 32768

[generation]
default_model = "zzz/reasoning-model"
"#;

/// A `.md` agent type naming `model` as its own (spec §5.5), written into the
/// project the parent session is created in — the root assembly discovers types
/// at launch, so the file has to precede `SessionNew`.
fn write_agent_type(project: &Path, name: &str, model: &str) {
    let dir = project.join(".tau").join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: reviews\nmodel: {model}\n---\nYou review.\n"),
    )
    .unwrap();
}

/// A parent session over the pair config, in `project`.
async fn parent_session(system: &Path, project: &Path) -> (Arc<Core>, String) {
    std::fs::write(system.join("config.toml"), CONFIG).unwrap();
    let core = CoreBuilder::default_system()
        .unwrap()
        .with_system_dir(system.to_path_buf())
        .with_home(system.to_path_buf())
        .build()
        .unwrap();
    let w = open_ws(&core, project).await;
    let id = match core
        .dispatch(Command::SessionNew {
            workspace: w.id.clone(),
            title: None,
            base_prompt: None,
        })
        .unwrap()
    {
        CommandOutput::Session { session } => session.id,
        other => panic!("expected a session: {other:?}"),
    };
    (core, id)
}

/// A child of `agent_type`, as its session id.
fn spawn(core: &Arc<Core>, parent: &str, agent_type: &str) -> String {
    match core
        .dispatch(Command::SubagentSpawn {
            session: parent.into(),
            agent_type: agent_type.into(),
            brief: "look".into(),
            context_mode: ContextMode::Fresh,
        })
        .unwrap()
    {
        CommandOutput::Subagent { subagent } => subagent.child,
        other => panic!("expected a subagent: {other:?}"),
    }
}

/// The child's first assistant entry as (text, reasoning): the child's turn
/// settles on its own, so the wait is bounded.
async fn child_answer(core: &Arc<Core>, project: &Path, child: &str) -> (String, String) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let running = core
            .sessions
            .lock()
            .get(child)
            .is_some_and(|l| l.turn.load(Ordering::SeqCst));
        let turns = assistant_entries(project, child);
        if !running && !turns.is_empty() {
            return turns[0].clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child's turn settled with no assistant entry"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn assistant_entries(project: &Path, id: &str) -> Vec<(String, String)> {
    let mut store = SessionStore::for_workspace(project, id);
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

/// The positive: a child whose agent type names a model of a provider **other
/// than** its parent's reaches that provider. The parent runs on `zzz`
/// (reasoning) and `aaa` is the map's first entry, so a text-only answer with no
/// reasoning is the child's own provider talking — the parent's endpoint cannot
/// produce it.
#[tokio::test]
async fn a_child_is_addressed_at_the_provider_its_model_names() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_agent_type(project.path(), "reviewer", "aaa/text-model");
    let (core, parent) = parent_session(system.path(), project.path()).await;
    let child = spawn(&core, &parent, "reviewer");
    let (text, reasoning) = child_answer(&core, project.path(), &child).await;
    assert!(
        text.starts_with("word 0"),
        "the provider the child's model names answered: {text:?}"
    );
    assert_eq!(
        reasoning, "",
        "the parent's reasoning script answered, not the child model's provider"
    );
}

/// The negative: a child model whose prefix names no configured provider keeps
/// the **parent's** route. Falling through to the map's first entry — `aaa`,
/// text-only — is what the parent's route must not do, so the reasoning script
/// answering is the proof the child stayed on `zzz`.
#[tokio::test]
async fn a_child_on_an_unroutable_model_keeps_its_parents_provider() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_agent_type(project.path(), "reviewer", "nosuch/model");
    let (core, parent) = parent_session(system.path(), project.path()).await;
    let child = spawn(&core, &parent, "reviewer");
    let (_text, reasoning) = child_answer(&core, project.path(), &child).await;
    assert!(
        reasoning.starts_with("thought 0"),
        "the child kept its parent's provider, not the config's first entry: {reasoning:?}"
    );
}

/// The inherit rule (#336) read off the parent's **current** model: `general`
/// names no model of its own, so a child of a parent that moved to another
/// provider's model is addressed at the provider that model names. The parent
/// opens on `zzz` (the reasoning script) and moves to `aaa` (text-only), so a
/// reasoning answer is the launch-time model talking.
#[tokio::test]
async fn a_child_inherits_the_model_its_parent_moved_to() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, parent) = parent_session(system.path(), project.path()).await;
    core.dispatch(Command::SessionSetModel {
        session: parent.clone(),
        model: "aaa/text-model".into(),
    })
    .unwrap();
    let child = spawn(&core, &parent, "general");
    let (text, reasoning) = child_answer(&core, project.path(), &child).await;
    assert!(
        text.starts_with("word 0"),
        "the provider the parent moved to answered the child: {text:?} / {reasoning:?}"
    );
    assert_eq!(
        reasoning, "",
        "the child ran the parent's launch-time provider"
    );
}

/// The rule's boundary: the move reaches a model-less type, not a type naming
/// its own model. `reviewer` names `zzz`'s model, so the reasoning script answers
/// while its parent runs text-only on `aaa`.
#[tokio::test]
async fn a_child_naming_its_own_model_ignores_the_parents_move() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_agent_type(project.path(), "reviewer", "zzz/reasoning-model");
    let (core, parent) = parent_session(system.path(), project.path()).await;
    core.dispatch(Command::SessionSetModel {
        session: parent.clone(),
        model: "aaa/text-model".into(),
    })
    .unwrap();
    let child = spawn(&core, &parent, "reviewer");
    let (text, reasoning) = child_answer(&core, project.path(), &child).await;
    assert!(
        reasoning.starts_with("thought 0"),
        "the type's own model answered, not the parent's new one: {text:?}"
    );
}

/// The supervisor a model-less child inherits from, as the live parent carries it.
fn parent_supervisor(core: &Arc<Core>, parent: &str) -> Arc<crate::subagent::Supervisor> {
    core.live(parent)
        .unwrap()
        .agent
        .subagents()
        .expect("a root session has a supervisor")
}

/// #337 × #336: the reload reaches the pair. The config lowers the cap on the
/// parent's model, the turn boundary takes the reloaded file, and the inherited
/// pair carries the new fact — a child priced with the launch-time cap is the bug.
#[tokio::test]
async fn a_config_move_reaches_the_pair_a_child_inherits() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, parent) = parent_session(system.path(), project.path()).await;
    let live = core.live(&parent).unwrap();
    let sup = parent_supervisor(&core, &parent);
    assert_eq!(sup.inherited().model, "zzz/reasoning-model");
    assert_eq!(sup.inherited().turn.max_output_tokens, Some(8192));

    assert!(CONFIG.contains("max_tokens = 8192"), "fixture drift");
    write_config_fixture(
        system.path(),
        &CONFIG.replace("max_tokens = 8192", "max_tokens = 4096"),
    );
    // The watcher's own leg is tests_config_reload's (#337); this leg is the
    // turn boundary's, so the reload is taken synchronously.
    core.reload_system_config();
    core.rederive_turn(&live);
    assert_eq!(
        sup.inherited().turn.max_output_tokens,
        Some(4096),
        "the reloaded fact reaches what a child inherits"
    );
}

/// Negatively: a model the boundary cannot route leaves the inherited pair on
/// the model that has facts — a refusal never becomes what a child is addressed
/// at, and the launch-time facts stay.
#[tokio::test]
async fn a_refused_model_keeps_the_pair_a_child_inherits() {
    let system = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let (core, parent) = parent_session(system.path(), project.path()).await;
    let live = core.live(&parent).unwrap();
    // The config-side refusal (#337): the session carries a model id the
    // config routes no provider for, so the boundary keeps its pair.
    live.agent.set_model("nope/model".into());
    core.rederive_turn(&live);

    let sup = parent_supervisor(&core, &parent);
    assert_eq!(
        sup.inherited().model,
        "zzz/reasoning-model",
        "a refused model never becomes what a child inherits"
    );
    assert_eq!(sup.inherited().turn.max_output_tokens, Some(8192));
}
