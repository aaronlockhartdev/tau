//! Agent types (spec §5.5): a named configuration — system prompt + tool
//! subset + model override + default context mode. The built-in `general`
//! inherits the session's tools/model; user types are `.md` files with
//! frontmatter, discovered from `~/.config/tau/agents/` (system) and
//! `{project}/.tau/agents/` (project wins on name collision).
use crate::subagent::ContextMode;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentType {
    pub name: String,
    pub description: String,
    /// The system prompt (the file body); `general`'s is empty — it
    /// inherits the session's prompt.
    pub body: String,
    /// A tool-subset restriction (tool names); absent = all the session's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub context_mode: ContextMode,
}

/// The always-present built-in type (spec §5.5): inherits the session's
/// tools and model, fresh is the default.
pub fn builtin_general() -> AgentType {
    AgentType {
        name: "general".into(),
        description: "The default type: the session's own prompt, tools, and model.".into(),
        body: String::new(),
        tools: None,
        model: None,
        context_mode: ContextMode::Fresh,
    }
}

/// Discover the registry: the built-in `general` first, then system
/// `agents/*.md`, then project `.tau/agents/*.md` (project wins on a name
/// collision, spec §5.5).
pub fn discover(system_dir: Option<&Path>, project_root: &Path) -> Vec<AgentType> {
    let mut types = vec![builtin_general()];
    if let Some(dir) = system_dir.map(|d| d.join("agents")) {
        for t in dir_of(&dir) {
            replace(&mut types, t);
        }
    }
    for t in dir_of(&project_root.join(".tau").join("agents")) {
        replace(&mut types, t);
    }
    types
}

fn replace(types: &mut Vec<AgentType>, t: AgentType) {
    if let Some(slot) = types.iter_mut().find(|x| x.name == t.name) {
        *slot = t;
    } else {
        types.push(t);
    }
}

fn dir_of(dir: &Path) -> Vec<AgentType> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        if e.path().extension().is_some_and(|x| x == "md")
            && let Ok(raw) = std::fs::read_to_string(e.path())
            && let Some(t) = parse(&raw)
        {
            out.push(t);
        }
    }
    out
}

/// A type file: `---` frontmatter (name / description / tools / model /
/// context_mode) over the system-prompt body.
fn parse(raw: &str) -> Option<AgentType> {
    let rest = raw.strip_prefix("---\n")?;
    let (fm, body) = rest.split_once("\n---")?;
    let body = body.trim_start_matches('\n').trim().to_owned();
    let get = |key: &str| -> Option<String> {
        fm.lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim().to_owned())
    };
    let name = get("name")?;
    Some(AgentType {
        name,
        description: get("description").unwrap_or_default(),
        body,
        tools: get("tools").filter(|v| !v.is_empty()).map(|v| {
            v.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        }),
        model: get("model").filter(|v| !v.is_empty()),
        // `context_mode` is canonical; the spec's phrasing is
        // "default context mode", so the alias is accepted too (review N5).
        context_mode: match get("context_mode")
            .or_else(|| get("default_context_mode"))
            .as_deref()
        {
            Some("compacted") => ContextMode::Compacted,
            Some("fork") => ContextMode::Fork,
            _ => ContextMode::Fresh,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, raw: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.md")), raw).unwrap();
    }

    const REVIEWER: &str = "---\nname: reviewer\ndescription: reviews code\ntools: read, bash\nmodel: test-model\n---\nYou review code strictly.\n";

    #[test]
    fn discovery_layers_system_and_project_with_project_wins() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let project = root.path().join("project");
        write(&system.join("agents"), "reviewer", REVIEWER);
        write(
            &project.join(".tau").join("agents"),
            "reviewer",
            "---\nname: reviewer\ndescription: project reviewer\n---\nProject prompt wins.\n",
        );
        write(
            &project.join(".tau").join("agents"),
            "writer",
            "---\nname: writer\n---\nWrite things.\n",
        );
        let types = discover(Some(&system), &project);
        assert_eq!(types[0].name, "general");
        let reviewer = types.iter().find(|t| t.name == "reviewer").unwrap();
        // The project file wins the collision: its description and body,
        // and none of the system file's fields.
        assert_eq!(reviewer.description, "project reviewer");
        assert_eq!(reviewer.body, "Project prompt wins.");
        assert!(reviewer.tools.is_none());
        assert!(reviewer.model.is_none());
        let writer = types.iter().find(|t| t.name == "writer").unwrap();
        assert_eq!(writer.body, "Write things.");
        assert_eq!(writer.context_mode, ContextMode::Fresh);
    }

    #[test]
    fn frontmatter_parses_tools_model_and_context_mode() {
        let t = parse(REVIEWER).unwrap();
        assert_eq!(t.name, "reviewer");
        assert_eq!(
            t.tools.as_deref(),
            Some(["read".to_owned(), "bash".to_owned()].as_slice())
        );
        assert_eq!(t.model.as_deref(), Some("test-model"));
        assert_eq!(t.body, "You review code strictly.");
        let t = parse("---\nname: c\ncontext_mode: compacted\n---\nbody").unwrap();
        assert_eq!(t.context_mode, ContextMode::Compacted);
        let t = parse("---\nname: c\ndefault_context_mode: fork\n---\nbody").unwrap();
        assert_eq!(t.context_mode, ContextMode::Fork);
    }

    #[test]
    fn a_file_without_frontmatter_is_not_a_type() {
        assert!(parse("just a note").is_none());
    }
}
