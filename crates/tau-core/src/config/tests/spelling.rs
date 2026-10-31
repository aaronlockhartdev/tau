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
fn a_model_key_that_names_a_configured_provider_is_refused() {
    let toml = r#"[providers.aaa]
base_url = "http://x/v1"

[providers.aaa.models."aaa/solo"]
context_window = 1000
"#;
    let err = load_from(toml, "").unwrap_err();
    assert!(
        matches!(
            &err,
            LoadError::ShadowedModelKey { provider, model, named }
                if provider == "aaa" && model == "aaa/solo" && named == "aaa"
        ),
        "{err}"
    );
}

#[test]
fn a_model_key_whose_prefix_names_no_provider_is_addressable_by_its_owner() {
    let toml = r#"[providers.bbb]
base_url = "http://x/v1"

[providers.bbb.models."aaa/solo"]
context_window = 1000
"#;
    let config = load_from(toml, "").expect("`aaa` is no provider, so the key is not shadowed");
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
/// a section wholesale, so which provider names are configured is a fact about
/// the merge. A per-layer check would refuse a config whose project layer removes
/// the shadowing provider, and accept one whose project layer adds it.
#[test]
fn a_provider_the_project_layer_removes_stops_shadowing_the_key() {
    let system = r#"[providers.aaa]
base_url = "http://x/v1"

[providers.aaa.models.solo]
context_window = 1000

[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models."aaa/solo"]
context_window = 1000
"#;
    let project = r#"[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models."aaa/solo"]
context_window = 1000
"#;
    let config = load_from(system, project).expect("the merged set names no provider `aaa`");
    assert_eq!(
        config
            .providers
            .keys()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
        vec!["bbb"]
    );
}

#[test]
fn a_provider_the_project_layer_adds_shadows_the_key() {
    let system = r#"[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models."aaa/solo"]
context_window = 1000
"#;
    let project = r#"[providers.aaa]
base_url = "http://x/v1"

[providers.aaa.models.solo]
context_window = 1000

[providers.bbb]
base_url = "http://y/v1"

[providers.bbb.models."aaa/solo"]
context_window = 1000
"#;
    assert!(
        load_from(system, "").is_ok(),
        "the system layer alone loads"
    );
    let err = load_from(system, project).unwrap_err();
    assert!(
        matches!(
            &err,
            LoadError::ShadowedModelKey { provider, model, named }
                if provider == "bbb" && model == "aaa/solo" && named == "aaa"
        ),
        "{err}"
    );
}
