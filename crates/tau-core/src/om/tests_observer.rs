use super::*;

#[test]
fn parse_observer_output_extracts_observations_and_response() {
    let raw = concat!(
        "<observations>",
        "* 🔴 (14:30) User prefers direct answers",
        "</observations>",
        "<suggested-response>",
        "Walk through the changes.",
        "</suggested-response>"
    );
    let parsed = parse_observer_output(raw);
    assert_eq!(
        parsed.observations,
        "* 🔴 (14:30) User prefers direct answers"
    );
    assert_eq!(parsed.suggested_response, "Walk through the changes.");
    assert!(!parsed.degenerate);
}

#[test]
fn parse_observer_output_falls_back_to_list_items() {
    let raw = "* line one\n* line two\nplain line";
    let parsed = parse_observer_output(raw);
    assert_eq!(parsed.observations, "* line one\n* line two");
}

#[test]
fn sanitize_truncates_overlong_lines_only() {
    let long = "x".repeat(10_001);
    let out = sanitize_observation_lines(&format!("ok line\n{long}"));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "ok line");
    assert!(lines[1].ends_with("… [truncated]"));
    assert_eq!(lines[1].len(), 10_000 + " … [truncated]".len());
}

#[test]
fn sanitize_truncates_char_safe_on_emoji_dense_lines() {
    // 10,001 emojis: the cut lands right after the 10,000th — no split
    // character, the upstream marker intact.
    let line = "🔴".repeat(10_001);
    let out = sanitize_observation_lines(&line);
    assert_eq!(out, format!("{} … [truncated]", "🔴".repeat(10_000)));
    assert!(!out.contains('\u{fffd}'));
    // Mixed line: the cut falls mid-line, not mid-character.
    let mixed = "🔴".repeat(9_990) + &"x".repeat(1_020); // 10,010 chars
    let out = sanitize_observation_lines(&mixed);
    assert_eq!(
        out,
        format!("{}{} … [truncated]", "🔴".repeat(9_990), "x".repeat(10))
    );
}

#[test]
fn degenerate_detection_flags_the_upstream_strategies() {
    // Strategy 1: one repeated 200-char period — the window sampler
    // sees every sampled window duplicated.
    let windowed = "w".repeat(200).repeat(60); // 12,000 chars
    let raw = format!("<observations>\n{windowed}\n</observations>");
    assert!(parse_observer_output(&raw).degenerate);

    // Strategy 2: a single line over 50k chars.
    let long_line = "z".repeat(50_001);
    let raw = format!("<observations>\n{long_line}\n</observations>");
    assert!(parse_observer_output(&raw).degenerate);

    // Strategy 3: the upstream production case — a 21-line block
    // repeated 62 times. Its ~700-char period falls in the window
    // sampler's aliasing blind spot, so only the exact-duplicate-line
    // strategy catches it.
    let block: Vec<String> = (0..21)
        .map(|i| format!("observation line {i} with some content"))
        .collect();
    let looped = block
        .iter()
        .cycle()
        .take(21 * 62)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(looped.chars().count() >= 2000);
    let raw = format!("<observations>\n{looped}\n</observations>");
    assert!(parse_observer_output(&raw).degenerate);

    // Normal mixed content is not degenerate.
    let normal = (0..120)
        .map(|i| format!("line {i} with distinct content {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(normal.chars().count() >= 2000);
    assert!(!parse_observer_output(&normal).degenerate);
}
