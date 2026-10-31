//! What a provider request is addressed by (§6.7). The session, its records and
//! the GUI carry the model id; the request body carries the config key, and the
//! two only agree through the address the config resolved.

use super::*;

use crate::agent::testkit::{ScriptedProvider, session_in, sse};
use crate::harness::SessionRole;

fn agent_on(
    dir: &std::path::Path,
    provider: Arc<ScriptedProvider>,
    model: &str,
    address: Option<(&str, &str)>,
) -> Arc<AgentSession> {
    AgentSession::launch(
        session_in(dir),
        SessionRole::Bare {
            provider,
            system_prompt: "be terse".into(),
            model: model.into(),
            tools: crate::tools::surface::bare_specs(),
            cwd: dir.to_path_buf(),
            turn: TurnConfig {
                address: address.map(|(id, key)| ModelAddress {
                    id: id.into(),
                    key: key.into(),
                }),
                ..Default::default()
            },
            tool_batch_on_force: ToolBatchPolicy::Complete,
            om: None,
            om_route: crate::agent::OmRoute::Session,
            gates: None,
        },
    )
    .unwrap()
}

async fn one_turn(
    dir: &std::path::Path,
    provider: Arc<ScriptedProvider>,
    model: &str,
    address: Option<(&str, &str)>,
) -> String {
    let provider_for_assert = provider.clone();
    let agent = agent_on(dir, provider, model, address);
    agent.send("hello", Lane::FollowUp);
    agent.process().await.unwrap();
    provider_for_assert
        .models
        .lock()
        .first()
        .cloned()
        .unwrap_or_default()
}

/// A menu-qualified id addresses the endpoint by the key its provider declares,
/// not by the id the session and its records carry.
#[tokio::test]
async fn a_qualified_id_sends_the_key_the_config_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    assert_eq!(
        one_turn(dir.path(), provider, "dev/m", Some(("dev/m", "m"))).await,
        "m",
        "the request names the declared key"
    );
}

/// A turn config addressed for one model never addresses another: a subagent
/// child inherits its parent's config and an agent type can override its model,
/// and then the child's own id is the address.
#[tokio::test]
async fn an_address_for_another_model_never_travels() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    assert_eq!(
        one_turn(dir.path(), provider, "review-model", Some(("dev/m", "m")),).await,
        "review-model",
        "the parent's key does not name the child's model"
    );
}

/// With no config behind the turn, the id addresses itself — the spelling a
/// hand-built turn carries.
#[tokio::test]
async fn an_unresolved_id_addresses_itself() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    assert_eq!(
        one_turn(dir.path(), provider, "gateway/gpt-5", None).await,
        "gateway/gpt-5"
    );
}
