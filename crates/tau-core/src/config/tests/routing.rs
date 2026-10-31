use super::*;
/// §6.7: the model id a session carries may name its provider (the model menu
/// spells it that way); the config declares models by bare key. A prefix naming
/// a configured provider is that provider; anything else is the key itself.
#[test]
fn model_key_resolves_the_menu_spelling_to_the_config_key() {
    let mut providers = BTreeMap::new();
    providers.insert(
        "dev".to_owned(),
        crate::config::Provider::with_model("http://127.0.0.1:9/v1", "qwen"),
    );
    let config = Config {
        providers,
        ..Default::default()
    };
    assert_eq!(config.model_key("dev/qwen"), "qwen");
    assert_eq!(config.model_key("qwen"), "qwen", "a bare key is its own");
    // A gateway hosts models whose own names carry a `/`; the prefix is not a
    // configured provider, so the key keeps it.
    assert_eq!(config.model_key("openai/gpt-5"), "openai/gpt-5");
    // Resolving a second time is a no-op: the address is stable.
    assert_eq!(config.model_key(config.model_key("dev/qwen")), "qwen");
}

/// The provider each spelling is addressed at: the prefix owning it, the sole
/// declaration of a bare key, and the current provider when a bare key says
/// nothing (#336).
#[test]
fn route_model_binds_the_provider_the_id_names() {
    let config = routing_config();
    let first = config
        .provider_for("aaa/solo")
        .expect("aaa is a configured provider");
    // Prefix owning: the prefix selects its provider, whatever the map's order.
    let zzz = config
        .route_model("zzz/zzz-model", first)
        .expect("zzz is a configured provider");
    assert_eq!(zzz.name, "zzz");
    assert_eq!(zzz.provider.base_url, "http://127.0.0.1:9/zzz/v1");
    // Bare key declared once: it routes to its one declarer, not to `current`.
    assert_eq!(
        config
            .route_model("solo", first)
            .expect("only aaa declares solo")
            .name,
        "aaa"
    );
    // Bare key declared by both: the prefix is what disambiguates, so the core
    // never guesses and the key stays where the session already is.
    assert_eq!(
        config
            .route_model("shared", zzz)
            .expect("a shared key is never a refusal")
            .name,
        "zzz",
        "a key two providers declare stays with the provider in use"
    );
    // Bare key no provider declares: the same stay-put rule, not a refusal.
    assert_eq!(
        config
            .route_model("undeclared", zzz)
            .expect("an undeclared key is never a refusal")
            .name,
        "zzz"
    );
}

/// The negative half of §6.7: a prefix naming no configured provider addresses
/// no endpoint, so it is refused. At HEAD it travelled to the provider the map
/// sorts first — the #336 burst of 400s.
#[test]
fn route_model_refuses_a_prefix_that_names_no_provider() {
    let config = routing_config();
    let first = config
        .provider_for("aaa/solo")
        .expect("aaa is a configured provider");
    let err = config
        .route_model("nosuch/model", first)
        .expect_err("no provider is named `nosuch`");
    assert_eq!(err, crate::config::UnownedPrefix("nosuch".to_owned()));
    // The refusal reads as the config's own words, naming the prefix.
    assert_eq!(
        err.to_string(),
        "model names no configured provider 'nosuch'"
    );
}

/// `provider_for` is the prefix read `route_model` and `switch_route` share: the
/// entry a configured prefix names, `None` for every other spelling.
#[test]
fn provider_for_reads_only_a_configured_prefix() {
    let config = routing_config();
    assert_eq!(
        config.provider_for("zzz/shared").map(|p| p.name),
        Some("zzz")
    );
    assert_eq!(
        config.provider_for("shared").map(|p| p.name),
        None,
        "a bare key names no provider"
    );
    assert_eq!(
        config.provider_for("nosuch/x").map(|p| p.name),
        None,
        "an unconfigured prefix is not a provider"
    );
}

/// The key the routed provider declares: its own prefix stripped, and the id
/// kept whole for every spelling the prefix rule leaves alone.
#[test]
fn model_key_for_strips_only_the_owning_prefix() {
    assert_eq!(Config::model_key_for("zzz", "zzz/zzz-model"), "zzz-model");
    assert_eq!(
        Config::model_key_for("zzz", "aaa/zzz-model"),
        "aaa/zzz-model",
        "a foreign prefix is part of the key"
    );
    assert_eq!(Config::model_key_for("zzz", "zzz-model"), "zzz-model");
    // A gateway key that carries `/` under a provider nobody declared is its
    // provider's key verbatim.
    assert_eq!(Config::model_key_for("zzz", "openai/gpt-5"), "openai/gpt-5");
    // The route and the key agree: the provider `route_model` picks declares the
    // key `model_key_for` returns for it.
    let config = routing_config();
    let zzz = config
        .route_model("zzz/shared", config.provider_for("aaa/solo").expect("aaa"))
        .expect("zzz is a configured provider");
    assert!(
        zzz.provider
            .models
            .contains_key(Config::model_key_for(zzz.name, "zzz/shared")),
        "the routed provider declares the routed id's key"
    );
}

/// §6.7's child rule: a child is addressed at the provider its own model names;
/// an id that names no provider keeps the parent's entry, never the map's first
/// entry (#336).
#[test]
fn route_child_follows_the_child_model_and_keeps_the_parent_on_a_refusal() {
    let config = routing_config();
    let parent = config
        .provider_for("aaa/solo")
        .expect("aaa is a configured provider");
    // A child whose agent type names another provider reaches that provider.
    assert_eq!(
        config.route_child("zzz/zzz-model", parent).base_url,
        "http://127.0.0.1:9/zzz/v1"
    );
    // A child id whose prefix names no provider is the spawn's already-resolved
    // id: it keeps the parent, not the provider the map sorts first.
    assert_eq!(
        config.route_child("nosuch/model", parent).base_url,
        "http://127.0.0.1:9/aaa/v1"
    );
    let parent = config
        .provider_for("zzz/zzz-model")
        .expect("zzz is a configured provider");
    assert_eq!(
        config.route_child("nosuch/model", parent).base_url,
        "http://127.0.0.1:9/zzz/v1",
        "the fallback is the parent, so a session lower in the map keeps its own"
    );
}

/// §6.7 for `[om].om_model` (#348): the memory passes are addressed at the
/// provider the id names, and the key asked of it is the prefix stripped.
#[test]
fn route_om_binds_the_memory_passes_to_the_provider_the_id_names() {
    let config = routing_config();
    let zzz = config
        .provider_for("zzz/zzz-model")
        .expect("zzz is a configured provider");
    // The prefix owns the id: routed there, whatever the map's order, and the
    // provider is asked for the stripped key.
    let owner = config.route_om("aaa/solo", zzz).expect("aaa is configured");
    assert_eq!(owner.name, "aaa");
    assert_eq!(owner.provider.base_url, "http://127.0.0.1:9/aaa/v1");
    assert_eq!(Config::model_key_for(owner.name, "aaa/solo"), "solo");
    // A bare key with one declarer is addressed at its declarer, not at the
    // provider the session happens to ride.
    assert_eq!(
        config
            .route_om("solo", zzz)
            .expect("only aaa declares solo")
            .name,
        "aaa"
    );
    // A contested key is answered by the provider already in use, when it
    // declares the key: the prefix is what would disambiguate, and there is
    // nothing to guess.
    assert_eq!(
        config
            .route_om("shared", zzz)
            .expect("zzz declares shared")
            .name,
        "zzz"
    );
}

/// The negative half of #348: every spelling that addresses no endpoint is a
/// refusal naming the config key and the id, never the session's own address.
#[test]
fn route_om_refuses_an_id_that_addresses_no_endpoint() {
    let config = routing_config();
    let zzz = config
        .provider_for("zzz/zzz-model")
        .expect("zzz is a configured provider");
    let prefix = config
        .route_om("nosuch/model", zzz)
        .expect_err("no provider is named `nosuch`");
    assert!(
        matches!(prefix, crate::config::OmRouteMiss::UnownedPrefix { .. }),
        "{prefix:?}"
    );
    assert!(
        prefix.to_string().contains("om.om_model"),
        "the refusal names the config key the author must edit: {prefix}"
    );
    // The footgun from the ticket: a base_url pasted into `om_model`. Its
    // `http:` prefix names no provider, so it is a refusal, not a call to a
    // endpoint the config never declared.
    let url = config
        .route_om("http://127.0.0.1:9/v1/model-x", zzz)
        .expect_err("`http:` is not a provider");
    assert!(
        matches!(url, crate::config::OmRouteMiss::UnownedPrefix { prefix: ref p, .. } if p == "http:"),
        "{url:?}"
    );
    // A prefix that owns the id, at a provider that does not declare the key.
    let undeclared = config
        .route_om("aaa/zzz-model", zzz)
        .expect_err("aaa declares no `zzz-model`");
    assert!(
        matches!(undeclared, crate::config::OmRouteMiss::Undeclared { provider: Some(ref p), .. } if p == "aaa"),
        "{undeclared:?}"
    );
    // A bare key no provider declares: no provider to name in the record.
    let nowhere = config
        .route_om("undeclared", zzz)
        .expect_err("nothing declares `undeclared`");
    assert!(
        matches!(
            nowhere,
            crate::config::OmRouteMiss::Undeclared { provider: None, .. }
        ),
        "{nowhere:?}"
    );
}

/// A bare key two providers declare, at a provider that declares neither, is a
/// guess the harness refuses to make — and the record has to name the whole
/// collision set, because it is the author's fix instruction (#348).
#[test]
fn route_om_names_every_provider_when_a_bare_key_is_contested() {
    let mut config = routing_config();
    config.providers.insert(
        "bbb".to_owned(),
        crate::config::Provider::with_model("http://127.0.0.1:9/bbb/v1", "other"),
    );
    let bbb = config
        .provider_for("bbb/other")
        .expect("bbb is a configured provider");
    let miss = config
        .route_om("shared", bbb)
        .expect_err("bbb declares no `shared`");
    assert!(
        matches!(miss, crate::config::OmRouteMiss::Ambiguous { .. }),
        "{miss:?}"
    );
    let shown = miss.to_string();
    assert!(
        shown.contains("aaa") && shown.contains("zzz"),
        "the refusal names both declarers so the fix is nameable: {shown}"
    );
    assert!(
        shown.contains("provider/key"),
        "and says how to name it: {shown}"
    );
}
