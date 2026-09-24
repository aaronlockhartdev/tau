use super::*;

use crate::om::testkit::*;

#[test]
fn extraction_instructions_match_upstream_byte_for_byte() {
    let ts = mastra_file("observer-agent.ts");
    let expected = ts_literal(&ts, "export const OBSERVER_EXTRACTION_INSTRUCTIONS = ");
    assert_eq!(
        OBSERVER_EXTRACTION_INSTRUCTIONS, expected,
        "extraction instructions drifted from third_party/mastra-om/observer-agent.ts"
    );
}

#[test]
fn output_format_matches_upstream_assembly() {
    let ts = mastra_file("observer-agent.ts");
    let legacy = ts_literal(&ts, "const legacyContinuationSections =");
    // The evaluated no-arg form: the template up to the extractor slot
    // (the legacy continuation sections spliced in, which is what the
    // `||` evaluates to with no extractors).
    let tpl_at = ts.find("Use priority levels:").expect("template");
    let open = ts[..tpl_at].rfind('`').expect("template open backtick");
    let close = tpl_at
        + ts[tpl_at..]
            .find("${extractorSections")
            .expect("extractor slot");
    let expected = format!("{}{}", &ts[open + 1..close], legacy);
    assert_eq!(
        OBSERVER_OUTPUT_FORMAT, expected,
        "output format drifted from the upstream buildObserverOutputFormat"
    );
}

#[test]
fn guidelines_match_upstream() {
    let ts = mastra_file("observer-agent.ts");
    let expected = ts_literal(&ts, "const OBSERVER_GUIDELINES =");
    assert_eq!(OBSERVER_GUIDELINES, expected);
}

#[test]
fn constants_match_upstream() {
    let ts = mastra_file("constants.ts");
    let prompt = ts_literal(&ts, "export const OBSERVATION_CONTEXT_PROMPT =");
    assert_eq!(OBSERVATION_CONTEXT_PROMPT, prompt);
    let instructions = ts_literal(&ts, "export const OBSERVATION_CONTEXT_INSTRUCTIONS =");
    assert_eq!(
        OBSERVATION_CONTEXT_INSTRUCTIONS,
        instructions.replace("${date}", "{date}")
    );
    let hint = ts_literal(&ts, "export const OBSERVATION_CONTINUATION_HINT =");
    assert_eq!(OBSERVATION_CONTINUATION_HINT, hint);
}

#[test]
fn observer_prompt_matches_upstream_template() {
    let ts = mastra_file("observer-agent.ts");
    // Non-multithreaded template: the SECOND occurrence of the opening
    // sentence (the multiThread branch shares it).
    let opening = "You are the memory consciousness of an AI assistant.";
    let first = ts.find(opening).expect("first occurrence");
    let rest = &ts[first + opening.len()..];
    let at = first + opening.len() + rest.find(opening).expect("second occurrence");
    let open = ts[..at].rfind('`').expect("template open");
    let close = at + ts[at..].find('`').expect("template close");
    let template = &ts[open + 1..close];
    assert_eq!(
        OBSERVER_PROMPT_TEMPLATE, template,
        "observer system template drifted from the upstream non-multithreaded branch"
    );
    // Slot filling must match `buildObserverSystemPrompt()` with no
    // extractors: constants, both enabled continuation sentences, no custom
    // instruction.
    let mut built = template
        .replace(
            "${OBSERVER_EXTRACTION_INSTRUCTIONS}",
            OBSERVER_EXTRACTION_INSTRUCTIONS,
        )
        .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
        .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES);
    for name in ["currentTaskEnabled", "suggestedResponseEnabled"] {
        let i = built.find(&format!("${{\n    {name}")).expect(name);
        let j = i + built[i..].find('}').expect("ternary end") + 1;
        let span = &built[i..j];
        let a = span.find('\'').expect("branch open");
        let b = span.find("'\n").expect("branch close");
        built = format!("{}{}{}", &built[..i], &span[a + 1..b], &built[j..]);
    }
    built = built.replace("${customInstructions}", "");
    assert_eq!(observer_system_prompt(), built);
}

#[test]
fn parse_observer_output_extracts_all_three_sections() {
    let raw = concat!(
        "<observations>",
        "* 🔴 (14:30) User prefers direct answers",
        "</observations>",
        "<current-task>",
        "Primary: auth",
        "</current-task>",
        "<suggested-response>",
        "Walk through the changes.",
        "</suggested-response>"
    );
    let parsed = parse_observer_output(raw);
    assert_eq!(
        parsed.observations,
        "* 🔴 (14:30) User prefers direct answers"
    );
    assert_eq!(parsed.current_task, "Primary: auth");
    assert_eq!(parsed.suggested_response, "Walk through the changes.");
    assert!(!parsed.degenerate);
}

#[test]
fn parse_observer_output_falls_back_to_list_items() {
    let raw = "* line one\n* line two\nplain line";
    let parsed = parse_observer_output(raw);
    assert_eq!(parsed.observations, "* line one\n* line two");
    assert_eq!(parsed.current_task, "");
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

#[test]
fn context_instructions_format_the_date_slot() {
    let ts = mastra_file("constants.ts");
    let upstream = ts_literal(&ts, "export const OBSERVATION_CONTEXT_INSTRUCTIONS =");
    assert_eq!(
        OBSERVATION_CONTEXT_INSTRUCTIONS,
        upstream.replace("${date}", "{date}")
    );
    let date = "Dec 4, 2025";
    assert_eq!(
        OBSERVATION_CONTEXT_INSTRUCTIONS.replace("{date}", date),
        upstream.replace("${date}", date)
    );
}
