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
        "[om]\nmodel = \"om\"\nobserve_threshold = {max}\n",
        max = u32::MAX
    );
    let config = load_from(&toml, "").unwrap();
    assert_eq!(config.om.observe_threshold, u64::from(u32::MAX));
    assert_eq!(config.om.reflect_threshold, 40_000);
    assert_eq!(config.om.buffer_increment, 6_000);
}

#[test]
fn om_threshold_above_u32_max_is_rejected() {
    // Each of the three u64 thresholds is checked; the error names the
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

#[test]
fn a_zero_om_threshold_is_rejected_naming_the_field() {
    // #409: a zero observe threshold drops the settle's plan gate — every
    // entry becomes a blocking LLM call at the turn end; a zero reflect
    // threshold collapses the ladder; a zero increment crosses a boundary
    // per entry. All three fail the load by name.
    for (field, toml) in [
        ("om.observe_threshold", "[om]\nobserve_threshold = 0\n"),
        ("om.reflect_threshold", "[om]\nreflect_threshold = 0\n"),
        ("om.buffer_increment", "[om]\nbuffer_increment = 0\n"),
    ] {
        match load_from(toml, "").unwrap_err() {
            LoadError::OmThresholdZero { field: got } => assert_eq!(got, field),
            other => panic!("expected a zero-threshold error for {field}, got {other:?}"),
        }
    }
    // The negative: the section alone loads on its defaults.
    let cfg = load_from("[om]\nmodel = \"m\"\n", "").unwrap();
    assert_eq!(cfg.om.observe_threshold, 30_000);
}

#[test]
fn a_layer_naming_the_retired_buffer_tokens_key_fails_naming_the_successor() {
    // #408: `buffer_tokens` is retired — any layer naming it fails the load,
    // and the error names the interval knob that replaced it. The bound check
    // is gone with the key; presence alone is the verdict.
    for toml in [
        "[om]\nbuffer_tokens = 6000\n",
        "[om]\nbuffer_tokens = 5000000000\n",
    ] {
        let err = load_from(toml, "").unwrap_err();
        assert!(
            matches!(err, LoadError::OmBufferTokensRetired { .. }),
            "retired key accepted: {err:?}"
        );
        assert!(
            err.to_string().contains("buffer_increment"),
            "the retired-key error does not name the successor: {err}"
        );
    }
    // The layer that omits the key loads with the interval knob alone.
    let config = load_from("[om]\nbuffer_increment = 4000\n", "").unwrap();
    assert_eq!(config.om.buffer_increment, 4_000);
}

/// The losing layer still fails the load (#408): the merge replaces `[om]`
/// wholesale, so a merged-config check would let a project section without
/// the retired key hide the system layer's intent. The read catches it and
/// names the file to edit.
#[test]
fn a_losing_layer_naming_the_retired_key_still_fails_the_load() {
    let sys = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let system_file = sys.path().join(CONFIG_FILE);
    std::fs::write(&system_file, "[om]\nbuffer_tokens = 6000\n").unwrap();
    std::fs::write(
        proj.path().join(CONFIG_FILE),
        "[om]\nobserve_threshold = 1000\n",
    )
    .unwrap();
    let err = load(sys.path(), proj.path()).unwrap_err();
    assert!(
        matches!(err, LoadError::OmBufferTokensRetired { .. }),
        "the winning layer hid the retired key: {err:?}"
    );
    assert_eq!(err.path(), Some(system_file.as_path()));
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

// ---- #407: the generation profiles and the `[om] model` rename ----

#[test]
fn the_om_model_key_is_renamed_naming_the_successor() {
    let err = load_from("[om]\nom_model = \"dev/m\"\n", "").unwrap_err();
    assert!(
        matches!(err, LoadError::OmModelRenamed { .. }),
        "the renamed key must fail the load: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("om_model") && msg.contains("model"),
        "the refusal names both the retired key and its successor: {msg}"
    );
    assert!(err.path().is_some(), "the refusal names the layer file");
    // The successor key loads.
    let config = load_from("[om]\nmodel = \"dev/m\"\n", "").unwrap();
    assert_eq!(config.om.model, "dev/m");
    // The read-time rule (#408): a losing layer's renamed key is invisible to
    // the merged config, so the check rides every layer's read.
    let err = load_from("[om]\nom_model = \"dev/m\"\n", "[om]\nmodel = \"x\"\n").unwrap_err();
    assert!(matches!(err, LoadError::OmModelRenamed { .. }));
}

#[test]
fn the_generation_profiles_parse_and_resolve_role_over_model_over_global() {
    let config = load_from(
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n\
         [providers.dev.models.m]\ncontext_window = 1000\n\n\
         [providers.dev.models.m.generation]\ntemperature = 0.2\nmax_tokens = 512\n\n\
         [providers.dev.models.m.generation.extra_body]\nthinkingBudget = 215\nterse = true\n\n\
         [om]\nmodel = \"dev/m\"\n\n\
         [om.generation]\ntemperature = 0.3\nthinking = \"off\"\n\n\
         [generation]\ntemperature = 0.9\n",
        "",
    )
    .unwrap();
    let model = config.providers["dev"].models["m"]
        .generation
        .clone()
        .expect("the per-model profile parses");
    assert_eq!(model.temperature, Some(0.2));
    assert_eq!(model.max_tokens, Some(512));
    assert_eq!(model.extra_body["thinkingBudget"], ExtraValue::Int(215));
    assert_eq!(model.extra_body["terse"], ExtraValue::Bool(true));
    assert_eq!(config.om.generation.temperature, Some(0.3));
    assert_eq!(config.om.generation.thinking, Some(ThinkingLevel::Off));
    // Resolution is key by key: the role's 0.3 wins, the model's 512 fills
    // the role's gap, the model's extra_body rides under the role.
    let resolved = config.om_generation();
    assert_eq!(resolved.temperature, Some(0.3));
    assert_eq!(resolved.max_tokens, Some(512));
    assert_eq!(resolved.thinking, Some(ThinkingLevel::Off));
    assert_eq!(resolved.extra_body["thinkingBudget"], ExtraValue::Int(215));
    assert_eq!(resolved.extra_body["terse"], ExtraValue::Bool(true));
}

#[test]
fn om_generation_falls_through_to_the_global_chain() {
    // No role keys, no model profile: the global `[generation]` sampling and
    // the `[thinking]` level reach the OM call.
    let config = load_from(
        "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n\
         [providers.dev.models.m]\ncontext_window = 1000\n\n\
         [generation]\ntemperature = 0.9\n\n[thinking]\nlevel = \"high\"\n\n\
         [om]\nmodel = \"dev/m\"\n",
        "",
    )
    .unwrap();
    let resolved = config.om_generation();
    assert_eq!(resolved.temperature, Some(0.9));
    assert_eq!(resolved.thinking, Some(ThinkingLevel::High));
    // An empty `[om] model` names no model: the chain is global only, and the
    // level is the `[thinking]` default.
    let config = load_from("[generation]\ntemperature = 0.9\n", "").unwrap();
    let resolved = config.om_generation();
    assert_eq!(resolved.temperature, Some(0.9));
    assert_eq!(resolved.thinking, Some(config.thinking.level));
}

#[test]
fn a_generation_profile_unknown_key_is_rejected() {
    assert!(
        load_from(
            "[providers.dev]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n\
             [providers.dev.models.m]\ncontext_window = 1000\n\n\
             [providers.dev.models.m.generation]\nvibe = \"warm\"\n",
            "",
        )
        .is_err(),
        "the per-model profile is deny_unknown_fields"
    );
    assert!(
        load_from(
            "[om]\nmodel = \"dev/m\"\n\n[om.generation]\nvibe = \"warm\"\n",
            ""
        )
        .is_err(),
        "the role profile is deny_unknown_fields"
    );
}

/// The read failure names its file: a layer path that exists and cannot be
/// read (a directory in its place) fails as the typed read error with the
/// path in the message — the session-open headline chains this cause, and
/// "which file" is the answer (#419).
#[test]
fn an_unreadable_layer_names_the_file_the_read_failed_on() {
    let sys = tempfile::tempdir().unwrap();
    std::fs::create_dir(sys.path().join(crate::config::CONFIG_FILE)).unwrap();
    let err = load(
        sys.path(),
        std::path::Path::new(crate::config::NO_PROJECT_LAYER),
    )
    .unwrap_err();
    assert!(
        matches!(&err, LoadError::Io { .. }),
        "the failure is the typed read error: {err}"
    );
    let text = err.to_string();
    assert!(
        text.contains("config.toml") && text.contains("reading"),
        "the message names the file: {text}"
    );
    assert_eq!(
        err.path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
        Some("config.toml".to_owned()),
        "path() carries the file for the headline's chain"
    );
}
