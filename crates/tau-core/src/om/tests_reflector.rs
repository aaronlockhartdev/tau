use super::*;

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
