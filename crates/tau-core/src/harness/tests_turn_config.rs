//! The turn-option resolution (spec §12, #35): config + model facts →
//! the session's `TurnConfig`.
use super::*;
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
        context_window: Some(32768),
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
    assert_eq!(turn.context_window, Some(32768));
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
    assert_eq!(turn.context_window, None);
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
