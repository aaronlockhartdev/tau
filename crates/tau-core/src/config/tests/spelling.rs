use super::*;
#[test]
fn a_provider_name_that_carries_a_slash_is_refused() {
    let toml = r#"[providers."aa/bb"]
base_url = "http://x/v1"

[providers."aa/bb".models.m1]
context_window = 1000
"#;
    let err = load_from(toml, "").unwrap_err();
    assert!(
        matches!(&err, LoadError::ProviderNameHasSlash { provider } if provider == "aa/bb"),
        "{err}"
    );
    // docs/config.md's load-failure list quotes these strings.
    assert_eq!(
        err.to_string(),
        "providers.aa/bb: a provider name must not contain '/'; an id's prefix is its provider's name"
    );
}

#[test]
fn a_provider_name_of_dots_and_dashes_loads() {
    let toml = r#"[providers."zzz-local.v2"]
base_url = "http://x/v1"

[providers."zzz-local.v2".models."deepseek/chat"]
context_window = 1000
"#;
    let config = load_from(toml, "").unwrap();
    assert!(
        config
            .providers
            .get("zzz-local.v2")
            .is_some_and(|p| p.models.contains_key("deepseek/chat"))
    );
}

#[test]
fn a_model_key_under_a_provider_of_its_own_prefix_loads() {
    // The gateway shape (#354): a vendor-hosting provider's keys carry that
    // vendor's prefix, and the provider may be named for the vendor. The ids
    // disambiguate, so the config loads.
    let toml = r#"[providers.openai]
base_url = "http://x/v1"

[providers.openai.models."openai/gpt-5"]
context_window = 1000

[providers.openai.models.gpt-5]
context_window = 1000
"#;
    let config = load_from(toml, "").expect("the standard gateway spelling loads");
    let openai = config
        .provider_for("openai/openai/gpt-5")
        .expect("openai is a configured provider");
    assert_eq!(
        config
            .route_model("openai/openai/gpt-5", openai)
            .expect("the prefix names openai")
            .name,
        "openai"
    );
    // §6.7 consumes the prefix once, so the full id asks for the declared key.
    assert_eq!(
        Config::model_key_for("openai", "openai/openai/gpt-5"),
        "openai/gpt-5",
        "the declared key is addressed by the full id"
    );
    assert_eq!(
        Config::model_key_for("openai", "openai/gpt-5"),
        "gpt-5",
        "the bare id names the tail key: §6.7 reads its prefix as the provider"
    );
}

#[test]
fn a_model_key_whose_prefix_names_no_provider_is_addressable_by_its_owner() {
    let toml = r#"[providers.bbb]
base_url = "http://x/v1"

[providers.bbb.models."aaa/solo"]
context_window = 1000
"#;
    let config = load_from(toml, "").expect("`aaa` is no provider, so the key is its own");
    let current = config
        .provider_for("bbb/aaa/solo")
        .expect("bbb is a configured provider");
    // The id the menu builds reaches the declared key: the prefix owns the
    // provider, the key past it is what was declared.
    assert_eq!(
        config
            .route_model("bbb/aaa/solo", current)
            .expect("bbb is configured")
            .name,
        "bbb"
    );
    assert_eq!(
        Config::model_key_for("bbb", "bbb/aaa/solo"),
        "aaa/solo",
        "the request is addressed by the declared key"
    );
    // The same string spelled bare names no endpoint: §6.7 reads `aaa` as a
    // provider, and none is configured.
    let err = config
        .route_model("aaa/solo", current)
        .expect_err("no provider is named `aaa`");
    assert_eq!(err, crate::config::UnownedPrefix("aaa".to_owned()));
}

#[test]
fn a_thinking_level_no_provider_declares_is_refused() {
    let toml = r#"[providers.aaa]
base_url = "http://x/v1"

[providers.aaa.models.solo]
context_window = 1000

[thinking.levels]
"zzz/solo" = "high"
"#;
    let err = load_from(toml, "").unwrap_err();
    assert!(
        matches!(&err, LoadError::LevelUndeclared { key }
            if key == "zzz/solo"),
        "{err}"
    );
    assert_eq!(
        err.to_string(),
        "thinking.levels.zzz/solo: no provider declares it as a model, so the level is never read"
    );
}

#[test]
fn a_thinking_level_of_a_declared_key_or_a_bare_key_loads() {
    let toml = r#"[providers.bbb]
base_url = "http://x/v1"

[providers.bbb.models."deepseek/chat"]
context_window = 1000

[thinking.levels]
"deepseek/chat" = "high"
bare = "off"
"#;
    let config = load_from(toml, "").expect("a level keyed by a declared model is read");
    assert_eq!(config.thinking.levels.len(), 2);
}

/// The rules read the **merged** config, as the #338 facts do: a layer replaces
/// a section wholesale, so which model keys are declared is a fact about the
/// merge. A per-layer check would accept a config whose project layer replaces
/// the providers and leaves a `[thinking].levels` key naming nothing.
#[test]
fn a_levels_key_the_project_layer_undeclares_is_refused() {
    let system = r#"[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models."deepseek/chat"]
context_window = 1000

[thinking.levels]
"deepseek/chat" = "high"
"#;
    let project = r#"[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models.other]
context_window = 1000
"#;
    assert!(
        load_from(system, "").is_ok(),
        "the system layer alone declares the level's model"
    );
    let err = load_from(system, project).unwrap_err();
    assert!(
        matches!(&err, LoadError::LevelUndeclared { key } if key == "deepseek/chat"),
        "{err}"
    );
}

#[test]
fn a_levels_key_the_project_layer_declares_loads() {
    let system = "[thinking.levels]\n\"deepseek/chat\" = \"high\"\n";
    let project = r#"[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models."deepseek/chat"]
context_window = 1000
"#;
    assert!(
        load_from(system, "").is_err(),
        "the system layer alone names no provider that declares the key"
    );
    load_from(system, project).expect("the project layer's provider declares it");
}
