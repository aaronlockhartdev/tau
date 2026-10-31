//! The turn-option resolution (spec §12, #35): config + model facts →
//! the session's `TurnConfig`.
use super::*;
use crate::agent::ContextGate;
use crate::config::ThinkingLevel;

fn setup(
    def: crate::config::ModelDef,
    generation: crate::config::Generation,
    thinking: crate::config::Thinking,
    retention: crate::config::CacheRetention,
) -> (Config, crate::config::Provider) {
    let mut provider = crate::config::Provider::with_model("http://127.0.0.1:9/v1", "m");
    provider.models.insert("m".to_owned(), def);
    let mut providers = BTreeMap::new();
    providers.insert("dev".into(), provider.clone());
    let config = Config {
        providers,
        generation,
        thinking,
        cache: crate::config::Cache {
            retention,
            ..Default::default()
        },
        ..Default::default()
    };
    (config, provider)
}

#[test]
fn model_facts_override_the_global_generation() {
    let def = crate::config::ModelDef {
        context_window: 32768,
        max_tokens: Some(8192),
        reasoning: Some(true),
        cost: None,
    };
    let generation = crate::config::Generation {
        temperature: Some(0.7),
        max_tokens: Some(4096),
        ..Default::default()
    };
    // A per-model level override wins over the default level.
    let thinking = crate::config::Thinking {
        level: ThinkingLevel::High,
        levels: BTreeMap::from([("m".to_owned(), ThinkingLevel::Medium)]),
        summary: true,
        ..Default::default()
    };
    let (config, provider) = setup(
        def,
        generation,
        thinking,
        crate::config::CacheRetention::Long,
    );
    let turn = derive_turn(&config, &provider, "m", "sess-1");
    assert_eq!(turn.max_output_tokens, Some(8192));
    assert_eq!(turn.context_window, 32768);
    assert_eq!(
        turn.reasoning,
        Some(crate::provider::ReasoningEffort::Medium)
    );
    assert!(turn.reasoning_summary);
    assert_eq!(turn.temperature, Some(0.7));
    assert_eq!(
        turn.prompt_cache,
        Some(("sess-1".to_owned(), crate::config::CacheRetention::Long))
    );
}

#[test]
fn a_model_without_facts_falls_back_to_the_globals() {
    let generation = crate::config::Generation {
        max_tokens: Some(4096),
        ..Default::default()
    };
    let thinking = crate::config::Thinking {
        level: ThinkingLevel::High,
        ..Default::default()
    };
    let (config, provider) = setup(
        crate::config::ModelDef::default(),
        generation,
        thinking,
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "m", "sess-1");
    // No model fact → no ceiling, so the High thinking budget (16384 default)
    // raises the 4096 global to leave room for thinking (#36).
    assert_eq!(turn.max_output_tokens, Some(20480));
    // #338 makes the window a required fact: the entry with none (the `0`
    // sentinel) gates nothing, which is why the load rejects it. `max_tokens`
    // is declared here, so the gap names only the window.
    assert_eq!(turn.context_window, 0);
    assert!(
        matches!(
            turn.context_gate(),
            ContextGate::Missing { reasons }
                if reasons == vec!["context_window".to_owned()]
        ),
        "a model with no declared window has no gate, named by the fact"
    );
    assert_eq!(turn.reasoning, Some(crate::provider::ReasoningEffort::High));
    assert!(!turn.reasoning_summary);
    assert_eq!(turn.temperature, None);
    assert!(turn.prompt_cache.is_none());
}

#[test]
fn a_custom_thinking_budget_drives_the_output_cap() {
    let generation = crate::config::Generation {
        max_tokens: Some(4096),
        ..Default::default()
    };
    // A user-set high budget (2048) replaces the 16384 default (#36).
    let thinking = crate::config::Thinking {
        level: ThinkingLevel::High,
        budgets: BTreeMap::from([("high".to_owned(), 2048)]),
        ..Default::default()
    };
    let (config, provider) = setup(
        crate::config::ModelDef::default(),
        generation,
        thinking,
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "m", "sess-1");
    assert_eq!(turn.max_output_tokens, Some(6144));
}

#[test]
fn a_non_reasoning_model_never_carries_the_effort() {
    let def = crate::config::ModelDef {
        reasoning: Some(false),
        ..Default::default()
    };
    let thinking = crate::config::Thinking {
        level: ThinkingLevel::High,
        ..Default::default()
    };
    let (config, provider) = setup(
        def,
        crate::config::Generation::default(),
        thinking,
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "m", "sess-1");
    assert_eq!(turn.reasoning, None);
}

#[test]
fn an_off_level_omits_the_effort() {
    let (config, provider) = setup(
        crate::config::ModelDef::default(),
        crate::config::Generation::default(),
        crate::config::Thinking::default(),
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "m", "sess-1");
    assert_eq!(turn.reasoning, None);
}

/// §6.7: the model menu names a model `provider/key`, the config declares it by
/// bare key. One read resolves the id, so the facts come from the key and the
/// request is addressed by it.
#[test]
fn a_menu_qualified_id_resolves_to_the_declared_key() {
    let def = crate::config::ModelDef {
        context_window: 131_072,
        max_tokens: Some(16_384),
        ..Default::default()
    };
    let (config, provider) = setup(
        def,
        crate::config::Generation::default(),
        crate::config::Thinking::default(),
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "dev/m", "sess-1");
    assert_eq!(
        turn.context_window, 131_072,
        "the declared window is found under the menu's id"
    );
    assert_eq!(turn.max_output_tokens, Some(16_384));
    assert_eq!(
        turn.address
            .as_ref()
            .map(|a| (a.id.as_str(), a.key.as_str())),
        Some(("dev/m", "m")),
        "the endpoint is addressed by the key, not the menu's id"
    );
    assert_eq!(turn.request_address("dev/m"), "m");
    assert!(
        matches!(turn.context_gate(), ContextGate::Ready { .. }),
        "a model whose facts are declared has a working gate"
    );
}

/// The resolution does not invent facts: an id whose key the config never
/// declared still reports the gate gap by name.
#[test]
fn a_menu_qualified_id_for_an_undeclared_key_still_names_the_gap() {
    let (config, provider) = setup(
        crate::config::ModelDef {
            context_window: 131_072,
            max_tokens: Some(16_384),
            ..Default::default()
        },
        crate::config::Generation::default(),
        crate::config::Thinking::default(),
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "dev/ghost", "sess-1");
    assert_eq!(turn.context_window, 0);
    assert_eq!(
        turn.address
            .as_ref()
            .map(|a| (a.id.as_str(), a.key.as_str())),
        Some(("dev/ghost", "ghost")),
        "the address is the key even for a model nobody declared"
    );
    assert!(
        matches!(
            turn.context_gate(),
            ContextGate::Missing { reasons }
                if reasons == vec!["context_window".to_owned()]
        ),
        "an undeclared key has no window, and the cap has a default"
    );
}

/// A turn config is inherited by a subagent child, whose agent type can
/// override its model; the parent's resolved key belongs to the parent's
/// model, so it must never address the child's (§6.7).
#[test]
fn an_address_resolved_for_one_model_never_names_another() {
    let (config, provider) = setup(
        crate::config::ModelDef {
            context_window: 131_072,
            max_tokens: Some(16_384),
            ..Default::default()
        },
        crate::config::Generation::default(),
        crate::config::Thinking::default(),
        crate::config::CacheRetention::None,
    );
    let turn = derive_turn(&config, &provider, "dev/m", "sess-1");
    assert_eq!(turn.request_address("dev/m"), "m");
    assert_eq!(
        turn.request_address("review-model"),
        "review-model",
        "a model this config never resolved addresses itself"
    );
    // A hand-built config has no address at all: the id addresses itself.
    assert_eq!(
        crate::agent::TurnConfig::default().request_address("any/id"),
        "any/id"
    );
}

/// A provider that hosts several vendors spells its model keys with a slash
/// (`deepseek/chat`). §6.7 answers that id at the owning provider and addresses
/// the request by the declared key, so the thinking level is keyed by the key —
/// the menu's id finds nothing. Rules a prefixed level by what a provider
/// declares, not by its prefix (#354).
#[test]
fn a_slash_carrying_id_carries_the_level_keyed_by_its_declared_key() {
    let mut provider =
        crate::config::Provider::with_model("http://127.0.0.1:9/v1", "deepseek/chat");
    provider.models.insert(
        "m".to_owned(),
        crate::config::ModelDef {
            context_window: 131_072,
            ..Default::default()
        },
    );
    let levels = BTreeMap::from([
        ("deepseek/chat".to_owned(), ThinkingLevel::High),
        ("m".to_owned(), ThinkingLevel::Low),
    ]);
    let config = Config {
        providers: BTreeMap::from([("dev".to_owned(), provider.clone())]),
        thinking: crate::config::Thinking {
            levels,
            ..Default::default()
        },
        ..Default::default()
    };
    let turn = derive_turn(&config, &provider, "dev/deepseek/chat", "sess-1");
    assert_eq!(
        turn.reasoning,
        Some(crate::provider::ReasoningEffort::High),
        "the level keyed by the declared key is the one that applies"
    );
    assert_eq!(turn.request_address("dev/deepseek/chat"), "deepseek/chat");
}
