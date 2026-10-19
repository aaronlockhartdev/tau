//! The plan-file substrate (design "Plan file & subagent model", spec
//! §5.3): a session-scoped plan file the model edits with its ordinary
//! file tools — the single source of truth for the session's work. The
//! core is the only machine reader: the file watcher parses on change,
//! and this parser is a **total function** — checkboxes are the only
//! hard requirement, section matching is fuzzy, and anything
//! unrecognized folds into notes. It never fails and never blocks.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tau_protocol::payload::ResumeContract;

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

    /// The observed completion (spec §5.2: `done` = plan complete and gate
    /// passed): every step checked. A plan with no steps is not complete —
    /// absence of observation is not proof of completion.
    #[must_use]
    pub fn complete(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.done)
    }
}

/// The wire mirror of the parsed plan (the snapshot's `LiveState.plan` and
/// the `plan_changed` event; ADR-0006 keeps the wire type in the protocol
/// crate): a field-for-field copy.
impl From<Plan> for tau_protocol::snapshot::Plan {
    fn from(plan: Plan) -> Self {
        tau_protocol::snapshot::Plan {
            steps: plan
                .steps
                .into_iter()
                .map(|s| tau_protocol::snapshot::PlanStep {
                    text: s.text,
                    done: s.done,
                })
                .collect(),
            criteria: plan.criteria,
            notes: plan.notes,
        }
    }
}

/// The plan's resume contract (spec §5.3, ADR-0001 revision: the task
/// record's resume contract re-homed to the plan file): the first unchecked
/// step is the next action. `None` when the plan is complete — or empty.
#[must_use]
pub fn resume_contract(plan: &Plan) -> Option<ResumeContract> {
    plan.steps.iter().find(|s| !s.done).map(|s| ResumeContract {
        next_action: s.text.clone(),
    })
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
    "# Plan\n\n## Steps\nMulti-step work: list its steps here before starting.\n\n## Criteria\n\n## Notes\n".to_owned()
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
/// the model can map the state onto the file it edits — plus the next
/// unchecked step, so the state dump doubles as the direction for the
/// next action.
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
    if let Some(next) = resume_contract(plan) {
        out.push_str("\n\nNext: ");
        out.push_str(&next.next_action);
    }
    out
}

/// The model-facing plan-file section heading. `with_path` targets the
/// section through `PLAN_SECTION_ANCHOR`, not this bare heading — a user
/// AGENTS.md may legitimately carry a literal `# Plan file`.
pub const PLAN_SECTION_HEADER: &str = "# Plan file";

/// The injected section's unambiguous anchor: the heading plus the fixed
/// intro line that always immediately follows it. `with_path` matches this
/// composite, not the bare heading, so a user AGENTS.md that carries a
/// literal `# Plan file` is never mistaken for the injected section — only
/// the text this code injects has the composite.
pub const PLAN_SECTION_ANCHOR: &str = "# Plan file\n\nYour plan lives at `";

/// The system-prompt plan-file section: the file's exact path plus the
/// concrete usage rules (the model edits it with its ordinary tools; the
/// core is its only machine reader). One paragraph: `with_path` bounds
/// the section at the first blank line after the header.
#[must_use]
pub fn section(file: &Path) -> String {
    format!(
        "{PLAN_SECTION_HEADER}\n\n\
         Your plan lives at `{}`. For multi-step work, list the steps in \
         the file before substantive work begins; tasks with fewer than two \
         steps need no plan. Keep steps to 5–7 imperative words, and check \
         a step off immediately when it is done — `- [ ]` becomes `- [x]` \
         the moment the step finishes, never batched to the end. When the \
         direction changes, edit the steps rather than abandoning them, and \
         keep the criteria and notes current. The current state, with the \
         next unchecked step, is re-injected into your context at every \
         turn.",
        file.display()
    )
}

/// The session's plan-file section in a system prompt: an existing
/// section (inherited from a parent's prompt) is replaced — a child's
/// prompt must name the child's own file, not the parent's. The
/// replacement is section-scoped: the layers that follow the section
/// (the skills catalog is the assembled prompt's last layer) are
/// preserved.
#[must_use]
pub fn with_path(prompt: &str, file: &Path) -> String {
    let section = section(file);
    let Some(start) = prompt.find(PLAN_SECTION_ANCHOR) else {
        return format!("{prompt}\n\n{section}");
    };
    // `find` indexes at a char boundary, so the slices are safe. The
    // section's body is a single paragraph, so the first layer separator
    // after the header's blank line bounds the old section — a
    // heading-only boundary would run to end-of-prompt and swallow the
    // skills catalog, which carries no `# ` heading of its own.
    let header_end = prompt
        .get(start..)
        .and_then(|rest| rest.find('\n').map(|i| start + i))
        .unwrap_or(prompt.len());
    let end = prompt
        .get(header_end + 2..)
        .and_then(|rest| rest.find("\n\n").map(|i| header_end + 2 + i))
        .unwrap_or(prompt.len());
    let mut out = prompt.get(..start).unwrap_or(prompt).to_owned();
    while out.ends_with(' ') || out.ends_with('\n') {
        out.pop();
    }
    out.push_str("\n\n");
    out.push_str(&section);
    if end < prompt.len() {
        out.push_str("\n\n");
        out.push_str(prompt.get(end + 2..).unwrap_or(""));
    }
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
#[path = "plan/tests.rs"]
mod tests;
