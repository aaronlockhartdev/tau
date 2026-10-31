use super::*;
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
