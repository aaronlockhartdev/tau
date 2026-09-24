use super::*;

#[test]
fn group_wrap_parse_round_trip() {
    let wrapped = wrap_in_observation_group(
        "* 🔴 (14:30) User prefers direct answers",
        "00000001:00000012",
        "0123456789abcdef",
        None,
    );
    let groups = parse_observation_groups(&wrapped);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id, "0123456789abcdef");
    assert_eq!(groups[0].range, "00000001:00000012");
    assert_eq!(groups[0].kind, None);
    assert_eq!(
        groups[0].content,
        "* 🔴 (14:30) User prefers direct answers"
    );
}

#[test]
fn group_kind_is_preserved() {
    let wrapped = wrap_in_observation_group("x", "a:b", "id1234567890abcd", Some("reflection"));
    assert_eq!(
        parse_observation_groups(&wrapped)[0].kind.as_deref(),
        Some("reflection")
    );
}

#[test]
fn multiple_groups_parse_in_order() {
    let log = format!(
        "{}\n\n{}",
        wrap_in_observation_group("one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None),
        wrap_in_observation_group("two", "00000006:00000010", "bbbbbbbbbbbbbbbb", None),
    );
    let groups = parse_observation_groups(&log);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].id, "aaaaaaaaaaaaaaaa");
    assert_eq!(groups[1].id, "bbbbbbbbbbbbbbbb");
    assert_eq!(strip_observation_groups(&log), "one\n\ntwo");
}

#[test]
fn combine_group_ranges_merges_a_span() {
    let groups = vec![
        ObservationGroup {
            id: "a".into(),
            range: "00000001:00000005".into(),
            kind: None,
            content: String::new(),
        },
        ObservationGroup {
            id: "b".into(),
            range: "00000006:00000010".into(),
            kind: None,
            content: String::new(),
        },
    ];
    assert_eq!(combine_group_ranges(&groups), "00000001:00000010");
}

#[test]
fn combine_group_ranges_collapses_to_first_start_last_end() {
    let groups = vec![ObservationGroup {
        id: "a".into(),
        range: "solo1,solo2".into(),
        kind: None,
        content: String::new(),
    }];
    assert_eq!(combine_group_ranges(&groups), "solo1:solo2");
    assert_eq!(combine_group_ranges(&[]), "");
}

#[test]
fn render_groups_for_reflection_emits_group_sections() {
    let log = wrap_in_observation_group(
        "* 🔴 (14:30) User prefers direct answers",
        "00000001:00000012",
        "0123456789abcdef",
        None,
    );
    let rendered = render_groups_for_reflection(&log).unwrap();
    assert_eq!(
        rendered,
        "## Group `0123456789abcdef`\n_range: `00000001:00000012`_\n\n* 🔴 (14:30) User prefers direct answers"
    );
    assert_eq!(
        render_groups_for_reflection("plain log").unwrap_or_default(),
        ""
    );
    assert!(render_groups_for_reflection("").is_none());
}

#[test]
fn derive_provenance_matches_by_shared_lines_and_merges_ranges() {
    let source = format!(
        "{}\n\n{}",
        wrap_in_observation_group(
            "* 🔴 (14:30) User prefers direct answers\n* 🟡 (14:31) Working on feature X",
            "00000001:00000005",
            "aaaaaaaaaaaaaaaa",
            None
        ),
        wrap_in_observation_group(
            "* 🔴 (09:15) Continued work on feature X",
            "00000006:00000010",
            "bbbbbbbbbbbbbbbb",
            None
        ),
    );
    let reflected = "## Group `aaaaaaaaaaaaaaaa`\n* 🔴 (14:30) User prefers direct answers\n* 🟡 (14:31) Working on feature X";
    let groups = parse_observation_groups(&source);
    let derived = derive_group_provenance(reflected, &groups);
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].id, "aaaaaaaaaaaaaaaa");
    assert_eq!(derived[0].range, "00000001:00000005");
    assert_eq!(derived[0].kind.as_deref(), Some("reflection"));
}

#[test]
fn derive_provenance_falls_back_to_index_when_lines_do_not_match() {
    let source = wrap_in_observation_group(
        "original line",
        "00000001:00000005",
        "aaaaaaaaaaaaaaaa",
        None,
    );
    let reflected = "## Group `cccccccccccccccc`\nrewritten line";
    let derived = derive_group_provenance(reflected, &parse_observation_groups(&source));
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].id, "cccccccccccccccc");
    assert_eq!(derived[0].range, "00000001:00000005");
}

#[test]
fn reconcile_rewraps_reflection_sections_as_groups() {
    let source = format!(
        "{}\n\n{}",
        wrap_in_observation_group("line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None),
        wrap_in_observation_group("line two", "00000006:00000010", "bbbbbbbbbbbbbbbb", None),
    );
    let reflected =
        "## Group `aaaaaaaaaaaaaaaa`\nline one\n\n## Group `bbbbbbbbbbbbbbbb`\nline two";
    let reconciled = reconcile_groups_from_reflection(reflected, &source).unwrap();
    let groups = parse_observation_groups(&reconciled);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].id, "aaaaaaaaaaaaaaaa");
    assert_eq!(groups[0].range, "00000001:00000005");
    assert_eq!(groups[0].kind.as_deref(), Some("reflection"));
    assert_eq!(groups[1].id, "bbbbbbbbbbbbbbbb");
    assert_eq!(groups[1].range, "00000006:00000010");
}

#[test]
fn reconcile_wraps_unsectioned_output_as_one_reflection_group() {
    let source =
        wrap_in_observation_group("line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None);
    let reconciled = reconcile_groups_from_reflection("plain rewritten log", &source).unwrap();
    let groups = parse_observation_groups(&reconciled);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].range, "00000001:00000005");
    assert_eq!(groups[0].kind.as_deref(), Some("reflection"));
}

#[test]
fn reconcile_without_source_groups_is_a_noop_and_empty_content_stays_empty() {
    assert_eq!(reconcile_groups_from_reflection("text", "plain log"), None);
    let source =
        wrap_in_observation_group("line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None);
    assert_eq!(
        reconcile_groups_from_reflection("", &source),
        Some(String::new())
    );
}

#[test]
fn generate_group_id_is_16_hex_and_deterministic() {
    let id = generate_group_id("00000001:00000005|some content");
    assert_eq!(id.len(), 16);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(id, generate_group_id("00000001:00000005|some content"));
}

#[test]
fn date_grouped_emoji_log_survives_the_group_round_trip() {
    // The log shape from the spec/ADR (date groups, emoji priorities, ✅
    // completions) must survive strip/wrap/parse without content change.
    let log = "Date: Dec 4, 2025\n* 🔴 (14:30) User prefers direct answers\n* 🟡 (14:32) User might prefer dark mode\n\nDate: Dec 5, 2025\n* 🔴 (09:15) Continued work on feature X\n* ✅ (09:40) Feature X shipped — user confirmed";
    let wrapped = wrap_in_observation_group(log, "00000001:00000020", "0123456789abcdef", None);
    assert_eq!(parse_observation_groups(&wrapped)[0].content, log);
    assert_eq!(strip_observation_groups(&wrapped), log);
}
