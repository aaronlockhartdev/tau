use super::*;

#[test]
fn defaults_match_mastra() {
    let c = OmConfig::default();
    assert_eq!(c.observe_threshold, 30_000);
    assert_eq!(c.reflect_threshold, 40_000);
    assert!(!c.share_token_budget);
}

#[test]
fn observer_fires_at_threshold_and_only_above() {
    let c = OmConfig::default();
    assert!(!should_observe(29_999, 0, &c));
    assert!(should_observe(30_000, 0, &c));
    assert!(should_observe(31_000, 0, &c));
}

#[test]
fn observer_threshold_is_configurable() {
    let c = OmConfig {
        observe_threshold: 10_000,
        ..Default::default()
    };
    assert!(!should_observe(9_999, 0, &c));
    assert!(should_observe(10_000, 0, &c));
}

#[test]
fn reflector_fires_at_threshold_and_only_above() {
    let c = OmConfig::default();
    assert!(!should_reflect(39_999, &c));
    assert!(should_reflect(40_000, &c));
}

#[test]
fn reflector_threshold_is_configurable() {
    let c = OmConfig {
        reflect_threshold: 5_000,
        ..Default::default()
    };
    assert!(!should_reflect(4_999, &c));
    assert!(should_reflect(5_000, &c));
}

#[test]
fn dynamic_threshold_uses_the_worked_example() {
    // The example in third_party/mastra-om/thresholds.ts (30k:40k, 70k total).
    let c = OmConfig {
        share_token_budget: true,
        ..Default::default()
    };
    assert_eq!(c.observe_threshold_for(0), 70_000);
    assert_eq!(c.observe_threshold_for(10_000), 60_000);
    assert_eq!(c.observe_threshold_for(40_000), 30_000);
    // Never below the base threshold.
    assert_eq!(c.observe_threshold_for(100_000), 30_000);
}

#[test]
fn observer_guard_shrinks_with_the_observation_log() {
    // With a shared budget the threshold shrinks as observations
    // accumulate: 35k pending is below the unshrunk 70k but at/above
    // the 30k floor reached at 40k observed.
    let c = OmConfig {
        share_token_budget: true,
        ..Default::default()
    };
    assert!(should_observe(35_000, 40_000, &c));
    assert!(!should_observe(29_999, 40_000, &c));
    // No shared budget: observed tokens never move the threshold.
    let c = OmConfig::default();
    assert!(should_observe(30_000, 40_000, &c));
    assert!(!should_observe(29_999, 40_000, &c));
}

#[test]
fn fixed_threshold_ignores_observation_size() {
    let c = OmConfig::default();
    assert_eq!(c.observe_threshold_for(0), 30_000);
    assert_eq!(c.observe_threshold_for(50_000), 30_000);
}

#[test]
fn buffer_increment_is_20_percent_of_the_message_threshold() {
    assert_eq!(OmConfig::default().buffer_increment(), 6_000);
    let c = OmConfig {
        observe_threshold: 20_000,
        buffer_activation: 0.75,
        ..Default::default()
    };
    assert_eq!(c.buffer_increment(), 5_000);
}

#[test]
fn retention_floor_is_ratio_and_absolute() {
    assert_eq!(OmConfig::default().retention_floor(), 6_000);
    let absolute = OmConfig {
        buffer_activation: 2_000.0,
        ..Default::default()
    };
    assert_eq!(absolute.retention_floor(), 2_000);
}

#[test]
fn projected_removal_prefers_the_nearest_over_boundary() {
    // 6k chunks (the ~6k increment); pending 30k, floor 6k → target 24k.
    let chunks = vec![ChunkTokens(6_000); 5];
    let c = OmConfig::default();
    assert_eq!(projected_message_removal(&chunks, &c, 30_000), 24_000);
}

#[test]
fn projected_removal_falls_back_under_when_overshoot_exceeds_95_percent_of_floor() {
    // Chunks 10k then 20k; pending 30k, floor 6k → target 24k. The over
    // boundary (30k) overshoots by 6k > 95% of the 6k floor (5.7k) → the
    // nearest under boundary (10k) wins.
    let chunks = vec![ChunkTokens(10_000), ChunkTokens(20_000)];
    let c = OmConfig::default();
    assert_eq!(projected_message_removal(&chunks, &c, 30_000), 10_000);
}

#[test]
fn projected_removal_noop_within_the_floor() {
    let chunks = vec![ChunkTokens(6_000); 5];
    let c = OmConfig::default();
    assert_eq!(projected_message_removal(&chunks, &c, 6_000), 0);
    assert_eq!(projected_message_removal(&[], &c, 30_000), 0);
}

#[test]
fn record_defaults_are_empty() {
    let r = OmRecord::default();
    assert!(r.frozen_prefix.is_empty());
    assert!(r.active_observations.is_empty());
    assert!(r.cursor.is_none());
    assert_eq!(r.generation, 0);
}

#[test]
fn frozen_prefix_survives_a_reflection_pass_verbatim() {
    let prefix = "Date: Nov 30, 2025\n* 🔴 (10:00) Frozen parent observation";
    let suffix = wrap_in_observation_group(
        "Date: Dec 4, 2025\n* 🔴 (14:30) User prefers direct answers",
        "00000001:00000005",
        "aaaaaaaaaaaaaaaa",
        None,
    );
    let mut record = OmRecord {
        frozen_prefix: prefix.to_owned(),
        active_observations: suffix.clone(),
        ..Default::default()
    };

    // The Reflector prompt is built from the suffix only — the frozen
    // prefix is not in its input.
    let prompt = build_reflector_prompt(record.reflect_source(), 1);
    assert!(!prompt.contains("Frozen parent observation"));
    assert!(prompt.contains("User prefers direct answers"));

    // The reflection output replaces only the suffix; the prefix stays
    // byte-verbatim and the live log is prefix + new suffix.
    let reflected =
        "## Group `aaaaaaaaaaaaaaaa`\nDate: Dec 4, 2025\n* 🔴 (14:30) User prefers direct answers";
    let reconciled = reconcile_groups_from_reflection(reflected, &suffix).unwrap();
    record.active_observations = reconciled;
    record.generation += 1;
    assert_eq!(record.frozen_prefix, prefix);
    assert_eq!(
        record.live_observations(),
        format!("{}{}", prefix, record.active_observations)
    );
}

#[test]
fn frozen_prefix_is_excluded_from_reflection_accounting() {
    // A frozen prefix large enough to trip the reflect threshold on its
    // own must not count: only the managed suffix's tokens drive the
    // trigger (ADR-0004: the prefix is excluded from OM accounting).
    let c = OmConfig {
        reflect_threshold: 100,
        ..Default::default()
    };
    let record = OmRecord {
        frozen_prefix: "x".repeat(400),      // 100 tokens on its own
        active_observations: "y".repeat(40), // 10 tokens
        observation_tokens: 10,
        ..Default::default()
    };
    assert!(!should_reflect(record.observation_tokens, &c));
}

#[test]
fn token_count_is_a_soft_quarter_estimate() {
    assert_eq!(token_count(""), 0);
    assert_eq!(token_count("abcd"), 1);
    assert_eq!(token_count(&"x".repeat(40_000)), 10_000);
}

#[test]
fn append_observation_adds_a_boundary_delimiter() {
    let first = "Date: Dec 4, 2025\n* 🔴 (14:30) User prefers direct answers";
    let appended = append_observation(
        first,
        "2025-12-05T09:15:00Z",
        "Date: Dec 5, 2025\n* 🔴 (09:15) Continued work on feature X",
    );
    assert!(appended.contains("--- message boundary (2025-12-05T09:15:00Z) ---"));
    assert!(appended.trim_end().ends_with('X'));
    let chunks = split_observation_chunks(&appended);
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].starts_with("Date: Dec 4, 2025"));
    assert!(chunks[1].starts_with("Date: Dec 5, 2025"));
}

#[test]
fn split_observation_chunks_drops_empty_segments() {
    let log = format!(
        "{}\n\n{}",
        "* 🟢 (10:00) info",
        message_boundary("2025-12-05T09:15:00Z")
    );
    assert_eq!(split_observation_chunks(&log).len(), 1);
    assert_eq!(split_observation_chunks("").len(), 0);
}
