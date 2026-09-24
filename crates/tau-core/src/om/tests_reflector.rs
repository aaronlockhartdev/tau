use super::*;

use crate::om::testkit::*;

fn reflector_template_from_ts() -> String {
    let ts = mastra_file("reflector-agent.ts");
    let at = ts
        .find("You are the memory consciousness of an AI assistant")
        .expect("reflector template");
    let open = ts[..at].rfind('`').expect("template open");
    let close = at + ts[at..].find('`').expect("template close");
    ts[open + 1..close].to_string()
}

fn fill_reflector_slots(template: &str) -> String {
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
    built.replace("${customInstructions}", "")
}

#[test]
fn reflector_template_matches_upstream() {
    assert_eq!(REFLECTOR_PROMPT_TEMPLATE, reflector_template_from_ts());
}

#[test]
fn reflector_system_prompt_fills_slots_like_upstream_default() {
    assert_eq!(
        reflector_system_prompt(),
        fill_reflector_slots(&reflector_template_from_ts())
    );
}

#[test]
fn compression_guidance_matches_upstream() {
    let ts = mastra_file("reflector-agent.ts");
    for (index, level) in COMPRESSION_GUIDANCE.iter().enumerate() {
        let marker = format!("  {}: `", index + 1);
        let at = ts.find(&marker).expect(&marker);
        let open = at + marker.len() - 1;
        let close = open + 1 + ts[open + 1..].find('`').expect("guidance close");
        assert_eq!(
            *level,
            &ts[open + 1..close],
            "compression level {} drifted",
            index + 1
        );
    }
}

#[test]
fn build_reflector_prompt_strips_groups_and_appends_guidance() {
    let log =
        wrap_in_observation_group("* line one", "00000001:00000005", "aaaaaaaaaaaaaaaa", None);
    let level0 = build_reflector_prompt(&log, 0);
    assert!(level0.contains("## OBSERVATIONS TO REFLECT ON"));
    assert!(level0.contains("* line one"));
    assert!(!level0.contains("<observation-group"));
    assert!(!level0.contains("COMPRESSION REQUIRED"));
    let level1 = build_reflector_prompt(&log, 1);
    assert!(level1.ends_with(COMPRESSION_GUIDANCE[0]));
    assert_ne!(level0, level1);
}
