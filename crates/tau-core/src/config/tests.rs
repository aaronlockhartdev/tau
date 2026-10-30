use super::*;

fn load_from(system: &str, project: &str) -> Result<Config, LoadError> {
    let (sys_dir, proj_dir) = (tempfile::tempdir()?, tempfile::tempdir()?);
    if !system.is_empty() {
        std::fs::write(sys_dir.path().join("config.toml"), system)?;
    }
    if !project.is_empty() {
        std::fs::write(proj_dir.path().join("config.toml"), project)?;
    }
    load(sys_dir.path(), proj_dir.path())
}

#[test]
fn absent_files_yield_defaults() {
    let config = load_from("", "").unwrap();
    assert!(config.providers.is_empty());
    assert_eq!(config.om.observe_threshold, 30_000);
    assert_eq!(config.om.reflect_threshold, 40_000);
    assert_eq!(config.om.buffer_increment, 6_000);
    assert_eq!(config.subagents.max_depth, 1);
    assert_eq!(config.requests.timeout_secs, 120);
    assert_eq!(config.requests.idle_timeout_secs, 120);
    assert_eq!(config.generation.default_model, "");
    assert_eq!(config.thinking.level, ThinkingLevel::Off);
    assert_eq!(config.cache.retention, CacheRetention::None);
    assert_eq!(config.limits.image.jpeg_quality, 80);
    assert_eq!(config.limits.image.max_pixels, 2_097_152);
    assert_eq!(config.limits.max_request_bytes, None);
    // No `[gates]` section: no gate (enforce-always — presence is the switch).
    assert!(config.gates.is_none());
}

#[test]
fn gates_section_parses_with_defaults_for_omitted_fields() {
    // The section's presence turns the gate on; omitted fields take their
    // defaults (timeout 300 s, max_retries 2).
    let toml = "[gates]\ncommand = \"cargo test\"\n";
    let config = load_from(toml, "").unwrap();
    let gates = config.gates.as_ref().unwrap();
    assert_eq!(gates.command, "cargo test");
    assert_eq!(gates.timeout_secs, 300);
    assert_eq!(gates.max_retries, 2);
}

#[test]
fn gates_section_overrides_defaults() {
    let toml = "[gates]\ncommand = \"just check\"\ntimeout_secs = 42\nmax_retries = 5\n";
    let config = load_from(toml, "").unwrap();
    let gates = config.gates.as_ref().unwrap();
    assert_eq!(gates.command, "just check");
    assert_eq!(gates.timeout_secs, 42);
    assert_eq!(gates.max_retries, 5);
}

#[test]
fn a_bare_gates_section_is_a_no_gate() {
    // An empty/blank command (the omitted key's serde default included) must
    // not turn the gate on as a silent no-op: `bash -c ""` exits 0.
    for toml in [
        "[gates]\n",
        "[gates]\ncommand = \"\"\n",
        "[gates]\ncommand = \"   \"\n",
    ] {
        let config = load_from(toml, "").unwrap();
        assert!(config.gates.is_none(), "expected no gate for: {toml}");
    }
}

#[test]
fn gates_unknown_key_is_rejected() {
    assert!(load_from("[gates]\ncommand = \"x\"\nbogus = 1\n", "").is_err());
}

#[test]
fn project_layer_overrides_system_layer() {
    let system = r#"
[providers.local]
base_url = "http://system:1/v1"
key_env = "SYS_KEY"

[providers.local.models."sys-model"]
context_window = 4096

[generation]
temperature = 0.2
"#;
    let project = r#"
[providers.local]
base_url = "http://project:2/v1"

[generation]
temperature = 0.9
"#;
    let config = load_from(system, project).unwrap();
    let local = &config.providers["local"];
    // Provider entries replace wholesale — a project entry is a unit, so
    // omitted fields fall back to their own defaults, not to system values.
    assert_eq!(local.base_url, "http://project:2/v1");
    assert_eq!(local.key_env, "");
    assert!(local.models.is_empty());
    assert_eq!(config.generation.temperature, Some(0.9));
}

#[test]
fn models_map_parses_with_optional_facts() {
    // The issue #35 example's shape: facts on one model, the optional facts
    // omitted on the other — the id is the key, and only the window is required
    // (#338).
    let toml = r#"
[providers.dev]
base_url = "https://llms.aaronlockhart.dev/v1"
key_env  = ""

[providers.dev.models."qwen3.8-27b"]
context_window = 32768
max_tokens     = 8192
reasoning      = true
cost           = { input = 0.5, output = 2.0 }

[providers.dev.models."qwen3.8-32b"]
context_window = 131072
"#;
    let config = load_from(toml, "").unwrap();
    let dev = &config.providers["dev"];
    assert_eq!(dev.models.len(), 2);
    assert_eq!(
        dev.models["qwen3.8-27b"],
        ModelDef {
            context_window: 32768,
            max_tokens: Some(8192),
            reasoning: Some(true),
            cost: Some(Cost {
                input: 0.5,
                output: 2.0
            })
        }
    );
    // The window is the one required fact; the rest stay at their defaults.
    assert_eq!(
        dev.models["qwen3.8-32b"],
        ModelDef {
            context_window: 131_072,
            ..ModelDef::default()
        }
    );
}

#[test]
fn a_model_with_no_context_window_fails_to_load() {
    // #338: the context gate is computed from the window, so an entry without
    // one leaves the session's prompts unbounded. The refusal names the
    // provider and model the user's edit must carry the fact on.
    for (provider, model, toml) in [
        (
            "dev",
            "m1",
            "[providers.dev]\nbase_url = \"http://127.0.0.1:1234/v1\"\n\n[providers.dev.models.\"m1\"]\n",
        ),
        (
            "dev",
            "m2",
            "[providers.dev]\nbase_url = \"http://127.0.0.1:1234/v1\"\n\n[providers.dev.models.\"m1\"]\ncontext_window = 4096\n\n[providers.dev.models.\"m2\"]\nmax_tokens = 1024\n",
        ),
    ] {
        let err = load_from(toml, "").unwrap_err();
        match err {
            LoadError::MissingContextWindow {
                provider: got_provider,
                model: got_model,
            } => {
                assert_eq!(got_provider, provider);
                assert_eq!(got_model, model);
            }
            other => panic!("expected the window error for {model}, got {other:?}"),
        }
    }
    // A window of 0 is the undeclared sentinel, not a tiny model.
    let zero = load_from(
        "[providers.dev]\nbase_url = \"http://x/v1\"\n\n[providers.dev.models.\"m1\"]\ncontext_window = 0\n",
        "",
    )
    .unwrap_err();
    assert!(matches!(zero, LoadError::MissingContextWindow { .. }));
}

#[test]
fn a_positive_output_cap_loads() {
    // The positive half of N6 (#338): a declared cap the window can carry is no
    // error, at a model entry or in the sampling defaults.
    let config = load_from(
        "[providers.dev]\nbase_url = \"http://x/v1\"\n\n[providers.dev.models.\"m1\"]\ncontext_window = 8192\nmax_tokens = 2048\n\n[generation]\nmax_tokens = 4096\n",
        "",
    )
    .unwrap();
    assert_eq!(
        config.providers["dev"].models["m1"].max_tokens,
        Some(2048),
        "the cap the user declared is the cap the gate wishes"
    );
    assert_eq!(config.generation.max_tokens, Some(4096));
}

#[test]
fn a_zero_output_cap_fails_to_load() {
    // N6 (#338): a cap of 0 arms `Ready { cap: 0 }`, so every call of a session
    // on it refuses at the wire with `context_room`. The config is the error, and
    // it names the entry the user's edit must fix — the dotted spelling, at both
    // spellings the cap exists at.
    for (key, toml) in [
        (
            "providers.dev.models.m1",
            "[providers.dev]\nbase_url = \"http://x/v1\"\n\n[providers.dev.models.\"m1\"]\ncontext_window = 8192\nmax_tokens = 0\n",
        ),
        (
            "generation",
            "[providers.dev]\nbase_url = \"http://x/v1\"\n\n[providers.dev.models.\"m1\"]\ncontext_window = 8192\n\n[generation]\nmax_tokens = 0\n",
        ),
    ] {
        let err = load_from(toml, "").expect_err("a cap of 0 is a config error");
        assert!(
            matches!(&err, LoadError::ZeroMaxTokens { key: got } if got == key),
            "expected the cap error for {key}, got {err:?}"
        );
        assert_eq!(
            err.to_string(),
            format!("{key} declares max_tokens = 0; the output cap must be positive")
        );
        // A validation failure is the merged config's property, not one file's.
        assert_eq!(err.path(), None, "the refusal names no layer (#337)");
    }
}

#[test]
fn a_declared_window_loads_with_the_rest_omitted() {
    // The positive half of the same rule: one fact, the window, is enough.
    let toml = r#"
[providers.dev]
base_url = "http://127.0.0.1:1234/v1"

[providers.dev.models."m1"]
context_window = 8192
"#;
    let config = load_from(toml, "").unwrap();
    assert_eq!(
        config.providers["dev"].models["m1"],
        ModelDef {
            context_window: 8192,
            ..ModelDef::default()
        }
    );
}

#[test]
fn full_layout_parses_and_layers() {
    let system = r#"
[providers.dev]
base_url = "https://llms.aaronlockhart.dev/v1"
key_env = "TAU_DEV_KEY"

[providers.dev.models."qwen3.8-27b"]
context_window = 32768
max_tokens     = 8192
reasoning      = true

[generation]
default_model = "qwen3.8-27b"
temperature = 0.7
max_tokens = 4096
top_p = 0.95
frequency_penalty = 0.1
presence_penalty = 0.0

[thinking]
level = "high"
summary = true

[thinking.levels]
"qwen3.5-9b" = "medium"

[thinking.budgets]
low = 1024
high = 8192

[cache]
retention = "long"
warming = "idle"

[requests]
timeout_secs = 60
retries = 3
idle_timeout_secs = 90
tool_batch_on_force = "kill"

[om]
om_model = "qwen3.8-32b"
observe_threshold = 1000

[subagents]
max_depth = 2
max_concurrent = 4

[limits]
max_request_bytes = 8388608

[limits.image]
max_width = 1024
max_height = 1024
max_bytes = 5242880
max_pixels = 1000000
jpeg_quality = 60
"#;
    let project = r#"
[cache]
retention = "short"
"#;
    let config = load_from(system, project).unwrap();
    // Untouched sections keep the system values.
    assert_eq!(config.thinking.level, ThinkingLevel::High);
    assert_eq!(config.thinking.levels["qwen3.5-9b"], ThinkingLevel::Medium);
    assert_eq!(config.thinking.budgets["high"], 8192);
    assert!(config.thinking.summary);
    // Project sections win wholesale (a section is a unit): the project's
    // [cache] replaces the system's, its omitted `warming` falling back to
    // its own default, not the system's.
    assert_eq!(config.cache.retention, CacheRetention::Short);
    assert_eq!(config.cache.warming, CacheWarming::Off);
    assert_eq!(config.generation.default_model, "qwen3.8-27b");
    assert_eq!(config.generation.max_tokens, Some(4096));
    assert_eq!(config.requests.timeout_secs, 60);
    assert_eq!(config.requests.idle_timeout_secs, 90);
    assert_eq!(config.requests.tool_batch_on_force, ToolBatchPolicy::Kill);
    assert_eq!(config.om.om_model, "qwen3.8-32b");
    assert_eq!(config.om.observe_threshold, 1000);
    assert_eq!(config.om.reflect_threshold, 40_000);
    assert_eq!(config.subagents.max_depth, 2);
    assert_eq!(config.subagents.max_concurrent, Some(4));
    assert_eq!(config.limits.max_request_bytes, Some(8_388_608));
    assert_eq!(config.limits.image.max_width, 1024);
    assert_eq!(config.limits.image.jpeg_quality, 60);
    assert_eq!(config.limits.image.max_pixels, 1_000_000);
}

#[test]
fn unknown_keys_are_rejected() {
    let err = load_from("", "[om]\nbogus = 1\n").unwrap_err();
    match err {
        LoadError::Parse(path, _) => assert!(path.ends_with("config.toml")),
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn unknown_keys_in_the_new_tables_are_rejected() {
    for toml in [
        "[generation]\nbogus = 1\n",
        "[thinking]\nbogus = 1\n",
        "[cache]\nbogus = 1\n",
        "[limits]\nbogus = 1\n",
        "[providers.dev]\nbase_url = \"http://x/v1\"\nbogus = 1\n",
        "[providers.dev.models.\"m\"]\nbogus = 1\n",
    ] {
        assert!(
            load_from(toml, "").is_err(),
            "expected a parse error for: {toml}"
        );
    }
}

#[test]
fn unknown_level_names_are_rejected() {
    assert!(load_from("", "[thinking]\nlevel = \"ultra\"\n").is_err());
    assert!(load_from("", "[cache]\nretention = \"eternal\"\n").is_err());
}

#[test]
fn om_thresholds_within_u32_load() {
    // The default layout sits far under u32::MAX; a value exactly at the
    // limit is still in-domain and must load (ticket #46).
    let toml = format!(
        "[om]\nom_model = \"om\"\nobserve_threshold = {max}\n",
        max = u32::MAX
    );
    let config = load_from(&toml, "").unwrap();
    assert_eq!(config.om.observe_threshold, u64::from(u32::MAX));
    assert_eq!(config.om.reflect_threshold, 40_000);
    assert_eq!(config.om.buffer_increment, 6_000);
}

#[test]
fn om_threshold_above_u32_max_is_rejected() {
    // Each of the four u64 thresholds is checked; the error names the
    // offending field and the limit (ticket #46).
    for (field, toml) in [
        (
            "om.observe_threshold",
            "[om]\nobserve_threshold = 5000000000\n",
        ),
        (
            "om.reflect_threshold",
            "[om]\nreflect_threshold = 5000000000\n",
        ),
        (
            "om.buffer_increment",
            "[om]\nbuffer_increment = 5000000000\n",
        ),
        ("om.buffer_tokens", "[om]\nbuffer_tokens = 5000000000\n"),
    ] {
        let err = load_from(toml, "").unwrap_err();
        match err {
            LoadError::OmThreshold {
                field: got,
                value,
                limit,
            } => {
                assert_eq!(got, field);
                assert_eq!(value, 5_000_000_000);
                assert_eq!(limit, u32::MAX);
            }
            other => panic!("expected an om threshold error for {field}, got {other:?}"),
        }
    }
}

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

/// The §6.7 routing set: two distinct providers, so a prefix has something to
/// select and a shared key has something to refuse to guess between. `aaa` sorts
/// first, so every assertion below that lands on `zzz` proves the route is not
/// the map's order (#336).
fn routing_config() -> Config {
    let mut aaa = crate::config::Provider::with_model("http://127.0.0.1:9/aaa/v1", "solo");
    aaa.models.insert("shared".to_owned(), ModelDef::default());
    let mut zzz = crate::config::Provider::with_model("http://127.0.0.1:9/zzz/v1", "zzz-model");
    zzz.models.insert("shared".to_owned(), ModelDef::default());
    Config {
        providers: BTreeMap::from([("aaa".to_owned(), aaa), ("zzz".to_owned(), zzz)]),
        ..Default::default()
    }
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

/// The rejected layer's file (#337): a parse failure names the layer the user
/// must edit, so `ConfigRejected` can carry the path.
#[test]
fn a_parse_failure_names_the_layer_file() {
    let sys = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let project_file = proj.path().join(CONFIG_FILE);
    std::fs::write(
        sys.path().join(CONFIG_FILE),
        "[providers.dev]\nbase_url = \"http://x/v1\"\n",
    )
    .unwrap();
    std::fs::write(&project_file, "[providers.dev]\nbase_url = ").unwrap();
    let err = load(sys.path(), proj.path()).unwrap_err();
    assert_eq!(err.path(), Some(project_file.as_path()));
}

/// A validation failure is the merged config's property, not one file's: it
/// names no layer, and the event's `path` is `null` (#337). The #338 window
/// refusal is that kind of failure: it names the provider and model to edit,
/// never a layer file.
#[test]
fn a_validation_failure_names_no_layer() {
    let sys = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    std::fs::write(
        sys.path().join(CONFIG_FILE),
        "[om]\nobserve_threshold = 5000000000\n",
    )
    .unwrap();
    let err = load(sys.path(), proj.path()).unwrap_err();
    assert!(matches!(err, LoadError::OmThreshold { .. }));
    assert_eq!(err.path(), None);
    let no_window = load_from(
        "[providers.dev]\nbase_url = \"http://x/v1\"\n\n[providers.dev.models.\"m1\"]\n",
        "",
    )
    .unwrap_err();
    assert!(matches!(no_window, LoadError::MissingContextWindow { .. }));
    assert_eq!(no_window.path(), None);
}
