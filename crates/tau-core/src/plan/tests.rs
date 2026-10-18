//! Tests for `crate::plan`.

use std::path::Path;

use super::*;

/// The seed skeleton is a parseable plan with no steps and empty
/// criteria; the invite line folds into notes.
#[test]
fn skeleton_parses_to_stepless_plan() {
    let plan = parse(&skeleton());
    assert_eq!(plan.steps.len(), 0);
    assert!(plan.criteria.trim().is_empty());
    assert!(plan.notes.contains("list its steps here"));
    assert!(!plan.complete());
}

/// The seed invite line is prose: it folds into notes, never a step.
#[test]
fn skeleton_invite_is_notes() {
    let plan = parse(&skeleton());
    assert!(
        plan.notes.contains("list its steps here"),
        "invite folds into notes: {plan:?}"
    );
    assert!(plan.steps.is_empty(), "invite is not a step: {plan:?}");
}

#[test]
fn parse_checks_steps_and_sections() {
    let raw = "# Plan\n\n## Steps\n- [x] a\n- [ ] b\n- [ ] c\n\n## Criteria\n\ndone when\n\n## Notes\nnote";
    let plan = parse(raw);
    assert_eq!(
        plan.steps,
        vec![
            PlanStep {
                text: "a".into(),
                done: true
            },
            PlanStep {
                text: "b".into(),
                done: false
            },
            PlanStep {
                text: "c".into(),
                done: false
            },
        ]
    );
    assert_eq!(plan.done_count(), 1);
    assert_eq!(plan.criteria.trim(), "done when");
    assert_eq!(plan.notes.trim(), "note");
}

#[test]
fn parse_crlf_is_equivalent() {
    let lf = parse("- [x] a\n- [ ] b");
    let crlf = parse("- [x] a\r\n- [ ] b\r\n");
    assert_eq!(lf, crlf);
}

#[test]
fn parse_fuzzy_criteria_heading() {
    let raw = "##Criteria\n\nc1\n\n# other\nc2";
    let plan = parse(raw);
    assert_eq!(plan.criteria.trim(), "c1");
    assert_eq!(plan.notes.trim(), "c2");
}

#[test]
fn parse_unrecognized_is_notes() {
    let plan = parse("any prose at all\n**bold**\n`code`");
    assert!(plan.steps.is_empty());
    assert_eq!(plan.notes.trim(), "any prose at all\n**bold**\n`code`");
}

#[test]
fn parse_garbage_does_not_panic() {
    let _ = parse("");
    let _ = parse("\n\n\n");
    let _ = parse("- [x]"); // no text after box
    let _ = parse("- [q] not a box");
    let _ = parse("####\t###");
}

#[test]
fn render_is_empty_header_for_empty_plan() {
    assert_eq!(render(&Plan::default()), "# Current plan state");
}

/// The re-injection carries the next unchecked step, and only one.
#[test]
fn render_names_next_step() {
    let plan = parse("- [x] a\n- [ ] b\n- [ ] c");
    let out = render(&plan);
    assert!(out.contains("\n\nNext: b"), "got: {out}");
    assert!(!out.contains("Next: c"));
}

/// A complete plan's re-injection has no nudge.
#[test]
fn render_complete_plan_has_no_next() {
    let plan = parse("- [x] a");
    assert!(!render(&plan).contains("Next:"));
}

#[test]
fn render_includes_criteria_and_notes() {
    let plan = parse("## Criteria\nc1\n## Notes\nn1");
    let out = render(&plan);
    assert!(out.contains("## Criteria\nc1"));
    assert!(out.contains("## Notes\nn1"));
}

#[test]
fn path_is_session_scoped() {
    let p = path(Path::new("/ws"), "abc123");
    assert_eq!(p, Path::new("/ws/.tau/plans/abc123.md"));
}

#[test]
fn resume_contract_is_first_open_step() {
    let plan = parse("- [x] a\n- [ ] b\n- [ ] c");
    let rc = resume_contract(&plan).expect("open step");
    assert_eq!(rc.next_action, "b");
}

#[test]
fn resume_contract_none_when_complete_or_empty() {
    assert!(resume_contract(&parse("- [x] a")).is_none());
    assert!(resume_contract(&Plan::default()).is_none());
}

#[test]
fn with_path_appends_when_absent() {
    let prompt = "You are Tau.\n\n## Skills\nx";
    let out = with_path(prompt, Path::new("/w/.tau/plans/s.md"));
    assert!(out.contains("Your plan lives at `/w/.tau/plans/s.md`"));
    assert!(out.contains("## Skills\nx"));
    assert!(out.starts_with("You are Tau."));
}

#[test]
fn with_path_replaces_inherited_section() {
    // Parent's section + the skills catalog that must survive.
    let parent = with_path("base", Path::new("/w/.tau/plans/parent.md"));
    let prompt = format!("{parent}\n\n# Skills\n- read\n- bash");
    let out = with_path(&prompt, Path::new("/w/.tau/plans/child.md"));
    assert!(out.contains("Your plan lives at `/w/.tau/plans/child.md`"));
    assert!(
        !out.contains("parent.md"),
        "child prompt must not name the parent's file: {out}"
    );
    assert!(
        out.contains("# Skills\n- read\n- bash"),
        "skills catalog must survive: {out}"
    );
    assert!(out.starts_with("base"));
}

#[test]
fn with_path_ignores_user_plan_file_heading() {
    // A user AGENTS.md may carry the bare heading; only the composite
    // anchor (heading + fixed intro) is the injected section's mark.
    let prompt = "# Plan file\n\nMy own notes about planning.\n\n# Skills\n- read";
    let out = with_path(prompt, Path::new("/w/.tau/plans/s.md"));
    assert!(out.contains("My own notes about planning."));
    assert!(out.contains("# Skills\n- read"));
    assert_eq!(
        out.matches("Your plan lives at").count(),
        1,
        "injected exactly once: {out}"
    );
}

#[test]
fn section_has_no_blank_line_inside() {
    // `with_path`'s boundary detection needs a single-paragraph body.
    let body = section(Path::new("/w/.tau/plans/s.md"));
    assert!(!body.contains("\n\n\n"));
    let after_header = body.split("\n\n").nth(1).expect("body after header");
    assert!(
        !after_header.contains('\n'),
        "body is one paragraph: {body}"
    );
}

#[test]
fn section_names_concrete_rules() {
    let s = section(Path::new("/w/.tau/plans/s.md"));
    assert!(s.contains("before substantive work begins"));
    assert!(s.contains("immediately when it is done"));
    assert!(s.contains("edit the steps"));
}

#[test]
fn session_of_matches_plans_dir_only() {
    let ws = Path::new("/ws");
    assert_eq!(
        session_of(Path::new("/ws/.tau/plans/a.md"), ws).as_deref(),
        Some("a")
    );
    assert_eq!(session_of(Path::new("/ws/.tau/plans/x/y.md"), ws), None);
    assert_eq!(session_of(Path::new("/ws/other/a.md"), ws), None);
    assert_eq!(session_of(Path::new("/ws/.tau/plans/a.txt"), ws), None);
    assert_eq!(session_of(Path::new("/ws/.tau/plans/.md"), ws), None);
}
