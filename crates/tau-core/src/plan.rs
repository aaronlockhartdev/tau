//! The plan-file substrate (design "Plan file & subagent model", spec
//! §5.3): a session-scoped plan file the model edits with its ordinary
//! file tools — the single source of truth for the session's work. The
//! core is the only machine reader: the file watcher parses on change,
//! and this parser is a **total function** — checkboxes are the only
//! hard requirement, section matching is fuzzy, and anything
//! unrecognized folds into notes. It never fails and never blocks.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// One plan step (a checkbox line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanStep {
    pub text: String,
    pub done: bool,
}

/// The parsed state of a plan file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// The checkbox steps, in file order.
    pub steps: Vec<PlanStep>,
    /// The fuzzy-matched `Criteria` section's body (empty when absent).
    pub criteria: String,
    /// Freeform notes: everything that is neither a step nor criteria.
    pub notes: String,
}

impl Plan {
    /// The checked-off step count.
    #[must_use]
    pub fn done_count(&self) -> usize {
        self.steps.iter().filter(|s| s.done).count()
    }
}

/// The session's plan file: `{workspace}/.tau/plans/{session-id}.md`.
#[must_use]
pub fn path(workspace: &Path, session_id: &str) -> PathBuf {
    workspace
        .join(".tau")
        .join("plans")
        .join(format!("{session_id}.md"))
}

/// The seed skeleton written at session start.
#[must_use]
pub fn skeleton() -> String {
    "# Plan\n\n## Steps\n\n## Criteria\n\n## Notes\n".to_owned()
}

/// Seed the session's plan file at session start (the design's
/// initializer pattern). Create-if-missing only — a re-opened session
/// must not clobber the model's plan. Best-effort: the plan file is an
/// aid, never a dependency of session start.
pub fn seed(workspace: &Path, session_id: &str) {
    let file = path(workspace, session_id);
    if file.exists() {
        return;
    }
    let Some(dir) = file.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("plan: cannot create {}: {e}", dir.display());
        return;
    }
    if let Err(e) = std::fs::write(&file, skeleton()) {
        tracing::warn!("plan: cannot seed {}: {e}", file.display());
    }
}

/// A markdown heading: one or more `#`, then end of line or any
/// continuation (`##Criteria` without the space counts — fuzzy, not a
/// spec). Returns the heading's title.
fn heading_title(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 {
        return None;
    }
    // `hashes` counts ASCII '#' bytes, so the index is a char boundary.
    Some(t.get(hashes..).unwrap_or(t).trim())
}

/// A checkbox line: a `-`/`*`/`+` bullet, a space, a `[` mark `]`, then
/// the step text. The mark is `x`/`X` (done) or blank/a single space
/// (open); any other mark content is prose, not a box.
fn checkbox(line: &str) -> Option<(bool, String)> {
    let t = line.trim_start();
    let rest = t
        .strip_prefix('-')
        .or_else(|| t.strip_prefix('*'))
        .or_else(|| t.strip_prefix('+'))?
        .strip_prefix(' ')?;
    let (mark, text) = rest.strip_prefix('[')?.split_once(']')?;
    if mark.len() > 2 {
        return None;
    }
    let done = matches!(mark, "x" | "X");
    if !done && !mark.is_empty() && mark != " " {
        return None;
    }
    let text = text.strip_prefix(' ').unwrap_or(text);
    Some((done, text.trim().to_owned()))
}

/// The total parser (the design's contract): checkbox lines are steps,
/// headings are structural (dropped), a fuzzy-matched `Criteria` heading
/// opens the criteria section (closed by the next heading), and
/// everything else folds into notes in file order. CRLF and LF are
/// equivalent; no input fails or panics.
#[must_use]
pub fn parse(raw: &str) -> Plan {
    let mut steps = Vec::new();
    let mut criteria = Vec::new();
    let mut notes = Vec::new();
    let mut in_criteria = false;
    for line in raw.lines() {
        if let Some(title) = heading_title(line) {
            in_criteria = title.to_ascii_lowercase().contains("criteria");
            continue;
        }
        if in_criteria {
            criteria.push(line.to_owned());
            continue;
        }
        if let Some((done, text)) = checkbox(line) {
            steps.push(PlanStep { text, done });
            continue;
        }
        notes.push(line.to_owned());
    }
    Plan {
        steps,
        criteria: criteria.join("\n"),
        notes: notes.join("\n"),
    }
}

/// The plan state as re-injected context (the design: re-injected at
/// each turn boundary and after compaction): the file's own shape, so
/// the model can map the state onto the file it edits.
#[must_use]
pub fn render(plan: &Plan) -> String {
    let mut out = String::from("# Current plan state");
    if !plan.steps.is_empty() {
        out.push_str("\n\n## Steps");
        for step in &plan.steps {
            out.push_str(if step.done { "\n- [x] " } else { "\n- [ ] " });
            out.push_str(&step.text);
        }
    }
    if !plan.criteria.trim().is_empty() {
        out.push_str("\n\n## Criteria\n");
        out.push_str(plan.criteria.trim());
    }
    if !plan.notes.trim().is_empty() {
        out.push_str("\n\n## Notes\n");
        out.push_str(plan.notes.trim());
    }
    out
}

/// The system-prompt plan-file section header: `with_path` replaces an
/// existing section at this marker.
pub const PLAN_SECTION_HEADER: &str = "# Plan file";

/// The system-prompt plan-file section: the file's exact path plus the
/// keep-current instruction (the model edits it with its ordinary
/// tools; the core is its only machine reader).
#[must_use]
pub fn section(file: &Path) -> String {
    format!(
        "{PLAN_SECTION_HEADER}\n\n\
         Your plan lives at `{}`. Keep it current as you work: check off \
         steps as they are done, and update the criteria and notes. The \
         current state is re-injected into your context at every turn.",
        file.display()
    )
}

/// The session's plan-file section in a system prompt: an existing
/// section (inherited from a parent's prompt) is replaced — a child's
/// prompt must name the child's own file, not the parent's.
#[must_use]
pub fn with_path(prompt: &str, file: &Path) -> String {
    let section = section(file);
    let start = prompt.find(PLAN_SECTION_HEADER).unwrap_or(prompt.len());
    // `find` indexes at a char boundary, so the slice is safe.
    let mut out = prompt.get(..start).unwrap_or(prompt).to_owned();
    while out.ends_with(' ') || out.ends_with('\n') {
        out.pop();
    }
    out.push_str("\n\n");
    out.push_str(&section);
    out
}

/// The session id a changed path names: a `.md` file directly under the
/// workspace's plans dir. `None` for nested paths, other extensions, and
/// paths outside the dir.
#[must_use]
pub(crate) fn session_of(file: &Path, workspace: &Path) -> Option<String> {
    let plans = workspace.join(".tau").join("plans");
    let rel = file.strip_prefix(&plans).ok()?;
    if rel.components().count() != 1 {
        return None;
    }
    let name = rel.file_name()?.to_string_lossy();
    name.strip_suffix(".md")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
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
    fn render_omits_empty_sections() {
        assert_eq!(render(&Plan::default()), "# Current plan state");
        let full = render(&well_formed());
        assert!(full.contains("- [x] investigate the failure"));
        assert!(full.contains("- [ ] fix the parser"));
        assert!(full.contains("## Criteria\n- build passes"));
        assert!(full.contains("## Notes"));
    }

    #[test]
    fn done_count_counts_checked_steps() {
        assert_eq!(well_formed().done_count(), 1);
        assert_eq!(Plan::default().done_count(), 0);
    }
}
