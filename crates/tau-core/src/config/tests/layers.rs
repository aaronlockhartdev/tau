use super::*;
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
