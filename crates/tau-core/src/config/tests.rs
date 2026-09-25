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
    assert_eq!(config.limits.max_request_bytes, None);
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
    // The issue #35 example, verbatim shape: facts on one model, none on the
    // other — the id is the key, an empty entry is fine.
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
"#;
    let config = load_from(toml, "").unwrap();
    let dev = &config.providers["dev"];
    assert_eq!(dev.models.len(), 2);
    assert_eq!(
        dev.models["qwen3.8-27b"],
        ModelDef {
            context_window: Some(32768),
            max_tokens: Some(8192),
            reasoning: Some(true),
            cost: Some(Cost {
                input: 0.5,
                output: 2.0
            })
        }
    );
    assert_eq!(dev.models["qwen3.8-32b"], ModelDef::default());
}

#[test]
fn defaults_when_facts_absent() {
    let toml = r#"
[providers.dev]
base_url = "http://127.0.0.1:1234/v1"

[providers.dev.models."m1"]
"#;
    let config = load_from(toml, "").unwrap();
    assert_eq!(config.providers["dev"].models["m1"], ModelDef::default());
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
