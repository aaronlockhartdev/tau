//! Tests for `crate::plan`.

use super::*;

const WELL_FORMED: &str = "\
# Plan

## Steps
- [x] investigate the failure
- [ ] fix the parser
- [ ] add tests

## Criteria
- build passes

## Notes
the parser is the culprit
";

fn well_formed() -> Plan {
    Plan {
        steps: vec![
            PlanStep {
                text: "investigate the failure".into(),
                done: true,
            },
            PlanStep {
                text: "fix the parser".into(),
                done: false,
            },
            PlanStep {
                text: "add tests".into(),
                done: false,
            },
        ],
        criteria: "- build passes\n".into(),
        notes: "\n\nthe parser is the culprit".into(),
    }
}

#[test]
fn a_well_formed_plan_parses_to_steps_criteria_and_notes() {
    assert_eq!(parse(WELL_FORMED), well_formed());
}

#[test]
fn an_empty_file_is_an_empty_plan() {
    assert_eq!(parse(""), Plan::default());
}

#[test]
fn crlf_lines_parse_like_lf() {
    let crlf = WELL_FORMED.replace('\n', "\r\n");
    assert_eq!(parse(&crlf), well_formed());
}

/// The mangled corpus (malformed checkboxes, missing/renamed `##
/// Criteria`, junk sections, empty file, CRLF, deep nesting).
fn corpus() -> Vec<(String, String)> {
    vec![
        ("well_formed".into(), WELL_FORMED.to_owned()),
        ("well_formed_crlf".into(), WELL_FORMED.replace('\n', "\r\n")),
        ("empty".into(), String::new()),
        (
            "malformed_checkboxes".into(),
            "- [x\n- [ ] ok\n* [X] upper\n+ [ ] plus\n- [ xx] junk\n- [] empty box\n- [ x] spaced\n".to_owned(),
        ),
        ("missing_criteria".into(), "# Plan\n- [ ] a\n- [ ] b\n".to_owned()),
        (
            "renamed_criteria".into(),
            "## Acceptance Criteria\ncargo nextest passes\n### criteria again\nmore\n".to_owned(),
        ),
        (
            "junk_sections".into(),
            "random line\n## Whatever\nsome body\n#title no space\n- [ ] a step\n".to_owned(),
        ),
        (
            "deep_nesting".into(),
            "- [ ] top\n  - [ ] mid\n    - [x] leaf\n      - [ ] deeper\n".to_owned(),
        ),
    ]
}

/// One corpus case's raw text by name.
fn corpus_raw(name: &str) -> String {
    corpus().into_iter().find(|(n, _)| n == name).unwrap().1
}

/// The total-function contract over the whole corpus: every mangled
/// shape parses and renders — no input fails or panics.
#[test]
fn the_mangled_corpus_parses_and_renders() {
    for (_, raw) in corpus() {
        let plan = parse(&raw);
        let _ = render(&plan);
    }
}

/// The snapshots are the record of what each mangled shape folds to.
#[test]
fn corpus_well_formed() {
    insta::assert_json_snapshot!(parse(&corpus_raw("well_formed")));
}

#[test]
fn corpus_well_formed_crlf() {
    insta::assert_json_snapshot!(parse(&corpus_raw("well_formed_crlf")));
}

#[test]
fn corpus_empty() {
    insta::assert_json_snapshot!(parse(&corpus_raw("empty")));
}

#[test]
fn corpus_malformed_checkboxes() {
    insta::assert_json_snapshot!(parse(&corpus_raw("malformed_checkboxes")));
}

#[test]
fn corpus_missing_criteria() {
    insta::assert_json_snapshot!(parse(&corpus_raw("missing_criteria")));
}

#[test]
fn corpus_renamed_criteria() {
    insta::assert_json_snapshot!(parse(&corpus_raw("renamed_criteria")));
}

#[test]
fn corpus_junk_sections() {
    insta::assert_json_snapshot!(parse(&corpus_raw("junk_sections")));
}

#[test]
fn corpus_deep_nesting() {
    insta::assert_json_snapshot!(parse(&corpus_raw("deep_nesting")));
}

#[test]
fn the_path_is_the_workspace_tau_plans_dir() {
    let p = path(Path::new("/w"), "abc");
    assert_eq!(p, PathBuf::from("/w/.tau/plans/abc.md"));
}

#[test]
fn seed_creates_the_skeleton_and_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    seed(ws, "s1");
    let file = path(ws, "s1");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), skeleton());
    std::fs::write(&file, "the model's plan").unwrap();
    seed(ws, "s1");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "the model's plan");
}

#[test]
fn session_of_names_only_plan_files_directly_under_plans() {
    let ws = Path::new("/w");
    assert_eq!(
        session_of(Path::new("/w/.tau/plans/abc.md"), ws).as_deref(),
        Some("abc")
    );
    assert_eq!(
        session_of(Path::new("/w/.tau/plans/archive/abc.md"), ws),
        None
    );
    assert_eq!(session_of(Path::new("/w/.tau/plans/abc.jsonl"), ws), None);
    assert_eq!(session_of(Path::new("/other/.tau/plans/abc.md"), ws), None);
    assert_eq!(session_of(Path::new("/w/.tau/plans/.md"), ws), None);
}

#[test]
fn with_path_appends_when_absent() {
    let out = with_path("base prompt", Path::new("/w/.tau/plans/abc.md"));
    assert!(out.starts_with("base prompt\n\n# Plan file"));
    assert!(out.contains("`/w/.tau/plans/abc.md`"));
}

#[test]
fn with_path_replaces_an_inherited_section() {
    let parent = with_path("base", Path::new("/w/.tau/plans/parent.md"));
    let child = with_path(&parent, Path::new("/w/.tau/plans/child.md"));
    assert!(child.contains("/w/.tau/plans/child.md"));
    assert!(!child.contains("parent.md"));
    assert_eq!(child.matches(PLAN_SECTION_HEADER).count(), 1);
}

#[test]
fn with_path_replaces_only_the_section_and_keeps_what_follows() {
    let parent = with_path("base", Path::new("/w/.tau/plans/parent.md"));
    let layered = format!("{parent}\n\n<available_skills>\n</available_skills>");
    let child = with_path(&layered, Path::new("/w/.tau/plans/child.md"));
    assert!(child.contains("/w/.tau/plans/child.md"));
    assert!(!child.contains("parent.md"));
    assert_eq!(child.matches(PLAN_SECTION_HEADER).count(), 1);
    // The layer after the section survives the replacement, still last.
    assert!(child.ends_with("<available_skills>\n</available_skills>"));
}

#[test]
fn with_path_does_not_truncate_user_content_named_like_the_section() {
    // A user AGENTS.md carrying a literal `# Plan file` heading: the
    // replacement must target the injected section (the anchor
    // composite), never the user's heading.
    let user = "base\n\n# Plan file\n\nthe user's own plan notes";
    let parent = with_path(user, Path::new("/w/.tau/plans/parent.md"));
    let child = with_path(&parent, Path::new("/w/.tau/plans/child.md"));
    // The user's content survives the replacement.
    assert!(child.contains("the user's own plan notes"));
    // The injected section is replaced with the child's path.
    assert!(child.contains("/w/.tau/plans/child.md"));
    assert!(!child.contains("parent.md"));
}

/// The re-injected shape is a contract (the model maps it onto the file):
/// the snapshot is the record.
#[test]
fn render_snapshot() {
    insta::assert_snapshot!(render(&well_formed()));
}

#[test]
fn render_omits_empty_sections() {
    assert_eq!(render(&Plan::default()), "# Current plan state");
    let full = render(&well_formed());
    assert!(full.contains("- [x] investigate the failure"));
    assert!(full.contains("- [ ] fix the parser"));
    assert!(full.contains("## Criteria\n- build passes"));
    assert!(full.contains("## Notes"));
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
fn done_count_counts_checked_steps() {
    assert_eq!(well_formed().done_count(), 1);
    assert_eq!(Plan::default().done_count(), 0);
}

/// The wire mirror is field-for-field (the protocol parity surface).
#[test]
fn the_wire_mirror_copies_every_field() {
    let wire: tau_protocol::snapshot::Plan = well_formed().into();
    assert_eq!(
        wire,
        tau_protocol::snapshot::Plan {
            steps: vec![
                tau_protocol::snapshot::PlanStep {
                    text: "investigate the failure".into(),
                    done: true,
                },
                tau_protocol::snapshot::PlanStep {
                    text: "fix the parser".into(),
                    done: false,
                },
                tau_protocol::snapshot::PlanStep {
                    text: "add tests".into(),
                    done: false,
                },
            ],
            criteria: "- build passes\n".into(),
            notes: "\n\nthe parser is the culprit".into(),
        }
    );
    let empty: tau_protocol::snapshot::Plan = Plan::default().into();
    assert_eq!(empty, tau_protocol::snapshot::Plan::default());
}

/// The observed `done` signal (spec §5.2): complete only when there are
/// steps and every one is checked.
#[test]
fn complete_requires_steps_and_all_checked() {
    // A partially checked plan is not complete.
    assert!(!well_formed().complete());
    // An empty plan (no steps) is not complete: absence of observation
    // is not proof of completion.
    assert!(!Plan::default().complete());
    // All steps checked: complete.
    let mut done = well_formed();
    for step in &mut done.steps {
        step.done = true;
    }
    assert!(done.complete());
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
fn section_has_no_blank_line_inside() {
    // `with_path`'s boundary detection needs a single-paragraph body.
    let body = section(Path::new("/w/.tau/plans/s.md"));
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
    assert!(s.contains("fewer than two steps"));
    assert!(s.contains("5–7 imperative words"));
    // The worked example: a check-off is a box flip, immediate.
    assert!(s.contains("`- [ ]` becomes `- [x]`"));
    assert!(s.contains("immediately"));
    assert!(s.contains("edit the steps"));
}
