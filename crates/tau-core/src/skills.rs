//! Skills (the spec's "Not in v0" entry; ticket #28): the Agent Skills
//! standard — a directory containing `SKILL.md` (YAML frontmatter + a
//! markdown body). Discovery is per workspace, project wins: the system
//! `~/.config/tau/skills/`, then the project's `.tau/skills/` and the
//! cross-client `.agents/skills/` convention. Validation is lenient per
//! the integrate guide: a missing description or unparseable frontmatter
//! skips the skill, a name violation only warns. A skill with
//! `disable-model-invocation: true` is excluded from the catalog — the
//! user's `/skill:` invocation is its only door.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Absolute path to the `SKILL.md` (the catalog's `location`; the
    /// skill directory is its parent).
    pub location: PathBuf,
    /// `false` for `disable-model-invocation: true` (the standard's
    /// filtering step): the skill is omitted from the catalog.
    pub model_invocation: bool,
}

/// Discover the workspace's registry: the system scope first, then the
/// project's (its `.tau/skills/`, then the cross-client
/// `.agents/skills/`). Project wins over system; within a scope the
/// first in the sorted scan wins and the shadow is logged.
pub fn discover(system_dir: Option<&Path>, project_root: &Path) -> Vec<Skill> {
    // Scope rank for collisions: project (1) beats system (0); within a
    // scope the first in the sorted scan wins — the shadow is logged.
    let mut reg: BTreeMap<String, (usize, Skill)> = BTreeMap::new();
    if let Some(dir) = system_dir.map(|d| d.join("skills")) {
        for s in scan(&dir) {
            insert(&mut reg, 0, s);
        }
    }
    for root in [
        project_root.join(".tau").join("skills"),
        project_root.join(".agents").join("skills"),
    ] {
        for s in scan(&root) {
            insert(&mut reg, 1, s);
        }
    }
    reg.into_values().map(|(_, s)| s).collect()
}

fn insert(reg: &mut BTreeMap<String, (usize, Skill)>, scope: usize, s: Skill) {
    match reg.get(&s.name) {
        Some((prev_scope, prev)) if *prev_scope >= scope => eprintln!(
            "skill {}: {} is shadowed by {} — skipped",
            s.name,
            s.location.display(),
            prev.location.display()
        ),
        _ => {
            if let Some(prev) = reg.get(&s.name) {
                eprintln!(
                    "skill {}: {} shadows {} — replaced",
                    s.name,
                    s.location.display(),
                    prev.1.location.display()
                );
            }
            reg.insert(s.name.clone(), (scope, s));
        }
    }
}

/// One skills root: the subdirectories (depth ≤ 4, `.git`/`node_modules`
/// skipped) holding a file named exactly `SKILL.md`, in sorted order. A
/// skill directory is a leaf — its `scripts/` and `references/` are
/// resources of that skill, not further skills.
fn scan(root: &Path) -> Vec<Skill> {
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

const MAX_DEPTH: usize = 4;
const SKIP: [&str; 2] = [".git", "node_modules"];

fn walk(dir: &Path, depth: usize, out: &mut Vec<Skill>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_some_and(|n| !SKIP.contains(&n.to_string_lossy().as_ref()))
        })
        .collect();
    dirs.sort();
    for d in dirs {
        if depth + 1 > MAX_DEPTH {
            continue;
        }
        let skill_md = d.join("SKILL.md");
        if skill_md.is_file()
            && let Ok(raw) = std::fs::read_to_string(&skill_md)
            && let Some(s) = parse(&raw, &skill_md)
        {
            out.push(s);
            continue;
        }
        walk(&d, depth + 1, out);
    }
}

/// A `SKILL.md`: `---` frontmatter over the body. Lenient per the
/// integrate guide: unparseable frontmatter skips the skill; a missing
/// description skips it; a name violation (uppercase, over 64, …)
/// warns but loads.
fn parse(raw: &str, location: &Path) -> Option<Skill> {
    let Some(fields) = frontmatter_fields(raw) else {
        eprintln!(
            "skill at {}: unparseable frontmatter — skipped",
            location.display()
        );
        return None;
    };
    let Some(name) = fields.get("name").cloned().filter(|n| !n.is_empty()) else {
        eprintln!(
            "skill at {}: no name in the frontmatter — skipped",
            location.display()
        );
        return None;
    };
    let description = fields.get("description").cloned().unwrap_or_default();
    if description.is_empty() {
        eprintln!(
            "skill {name} at {}: missing or empty description — skipped",
            location.display()
        );
        return None;
    }
    if !valid_name(&name) {
        eprintln!(
            "skill {name}: the name violates 1–64 of [a-z0-9-] with no leading/trailing/consecutive hyphens — loaded anyway"
        );
    }
    Some(Skill {
        name,
        description,
        location: location.to_path_buf(),
        model_invocation: !fields
            .get("disable-model-invocation")
            .is_some_and(|v| v == "true"),
    })
}

fn valid_name(n: &str) -> bool {
    (1..=64).contains(&n.len())
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !n.starts_with('-')
        && !n.ends_with('-')
        && !n.contains("--")
}

/// The frontmatter as `key: value` fields. The strict pass mirrors a YAML
/// parser (an unquoted value containing a colon fails it); the retry
/// accepts such values as quoted literals — the integrate guide's
/// cross-client fallback. A line with no colon at all fails both.
fn frontmatter_fields(raw: &str) -> Option<BTreeMap<String, String>> {
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    let (fm, _) = rest.split_once("\n---")?;
    let lines: Vec<&str> = fm
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with('#') && !l.starts_with([' ', '\t'])
        })
        .collect();
    match parse_fields(lines.clone(), false) {
        Some(fields) => Some(fields),
        None => parse_fields(lines, true),
    }
}

fn parse_fields(lines: Vec<&str>, allow_colons: bool) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for l in lines {
        let (k, v) = l.split_once(':')?;
        let v = v.trim();
        if !allow_colons && !v.starts_with(['"', '\'']) && v.contains(':') {
            return None;
        }
        out.insert(k.trim().to_owned(), unquote(v));
    }
    Some(out)
}

fn unquote(v: &str) -> String {
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1].to_owned()
    } else {
        v.to_owned()
    }
}

/// The body of a `SKILL.md`: everything after the closing `---`, trimmed.
pub fn body(raw: &str) -> String {
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"));
    match rest {
        Some(r) => r
            .split_once("\n---")
            .map(|(_, b)| b.trim().to_owned())
            .unwrap_or_else(|| r.trim().to_owned()),
        None => raw.trim().to_owned(),
    }
}

/// The `/skill:<name>` expansion (ticket #28): the invocation is recorded
/// as a user entry with this text — the body under a header naming the
/// skill directory; `args` (when present) ride the final line.
pub fn expand(name: &str, body: &str, dir: &Path, args: Option<&str>) -> String {
    let mut out = format!(
        "Skill `{name}` — follow the instructions below. The skill directory is {}; resolve relative paths in the instructions against it.\n\n{body}",
        dir.display()
    );
    if let Some(a) = args.map(str::trim).filter(|a| !a.is_empty()) {
        out.push_str(&format!("\n\nUser request: {a}"));
    }
    out
}

/// The tier-1 disclosure (the standard's `<available_skills>` block plus
/// its file-read behavioral instruction): `None` when no
/// model-invocable skill exists — the prompt omits the block entirely.
pub fn catalog(skills: &[Skill]) -> Option<String> {
    let listed: Vec<&Skill> = skills.iter().filter(|s| s.model_invocation).collect();
    if listed.is_empty() {
        return None;
    }
    let mut xml = String::from("<available_skills>\n");
    for s in listed {
        xml.push_str(&format!(
            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n",
            s.name,
            s.description,
            s.location.display()
        ));
    }
    xml.push_str("</available_skills>");
    Some(format!(
        "The following skills provide specialized instructions for specific tasks.\n\
         When a task matches a skill's description, use your file-read tool to load\n\
         the SKILL.md at the listed location before proceeding.\n\
         When a skill references relative paths, resolve them against the skill's\n\
         directory (the parent of SKILL.md) and use absolute paths in tool calls.\n\n{xml}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `root` is a system dir (the helper joins `skills/`) or a project
    /// root (relatives like `.tau/skills/x`).
    fn write_skill(root: &Path, rel: &str, raw: &str) {
        let path = if rel.starts_with('.') {
            root.join(rel)
        } else {
            root.join("skills").join(rel)
        };
        let path = path.join("SKILL.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, raw).unwrap();
    }

    fn skill(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\nDo {name}.\n")
    }

    #[test]
    fn discovery_resolves_all_locations_with_project_wins() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("system");
        let project = root.path().join("project");
        // The same name in all three scopes: the project's `.tau` root is
        // scanned first, then `.agents`; both beat the system.
        write_skill(&system, "shared", &skill("shared", "system shared"));
        write_skill(
            &project,
            ".tau/skills/shared",
            &skill("shared", "tau shared"),
        );
        write_skill(
            &project,
            ".agents/skills/shared",
            &skill("shared", "agents shared"),
        );
        write_skill(
            &project,
            ".tau/skills/tau-skill",
            &skill("tau-skill", "from .tau"),
        );
        write_skill(
            &project,
            ".agents/skills/agents",
            &skill("agents-skill", "from .agents"),
        );
        let skills = discover(Some(&system), &project);
        let get = |n: &str| skills.iter().find(|s| s.name == n).unwrap();
        assert_eq!(get("shared").description, "tau shared");
        assert_eq!(get("tau-skill").description, "from .tau");
        assert_eq!(get("agents-skill").description, "from .agents");
        assert_eq!(skills.len(), 3);
    }

    #[test]
    fn within_a_scope_the_first_in_the_sorted_scan_wins() {
        let root = tempfile::tempdir().unwrap();
        // `b` declares the name first in write order; `a` sorts first.
        write_skill(root.path(), ".agents/skills/b", &skill("dup", "second"));
        write_skill(root.path(), ".agents/skills/a", &skill("dup", "first"));
        let skills = discover(None, root.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "first");
    }

    #[test]
    fn depth_and_skip_rules() {
        let root = tempfile::tempdir().unwrap();
        // Depth 4 from the skills root: discovered. Depth 5: not.
        write_skill(
            root.path(),
            ".agents/skills/a/b/c/ok",
            &skill("deep-ok", "d4"),
        );
        write_skill(
            root.path(),
            ".agents/skills/a/b/c/d/too-deep",
            &skill("too-deep", "d5"),
        );
        // Skipped directory names inside the skills root.
        write_skill(
            root.path(),
            ".agents/skills/.git/x",
            &skill("in-git", "skip"),
        );
        write_skill(
            root.path(),
            ".agents/skills/node_modules/x",
            &skill("in-nm", "skip"),
        );
        // A SKILL.md at the root of the skills dir is not a skill.
        let p = root.path().join(".agents").join("skills");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("SKILL.md"), skill("root-md", "no")).unwrap();
        let skills = discover(None, root.path());
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["deep-ok"]);
    }

    #[test]
    fn missing_description_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        write_skill(root.path(), ".tau/skills/s", "---\nname: s\n---\nBody.\n");
        assert!(discover(None, root.path()).is_empty());
    }

    #[test]
    fn unparseable_frontmatter_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".tau/skills/s",
            "---\nname s\ndescription: d\n---\nBody.\n",
        );
        assert!(discover(None, root.path()).is_empty());
    }

    #[test]
    fn an_unquoted_colon_value_loads_via_the_quoted_retry() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".tau/skills/s",
            "---\nname: s\ndescription: Use this when: the user asks for it\n---\nBody.\n",
        );
        let skills = discover(None, root.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "Use this when: the user asks for it");
    }

    #[test]
    fn a_name_violation_warns_but_loads() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".tau/skills/s",
            "---\nname: PDF-Processing\ndescription: d\n---\nBody.\n",
        );
        let skills = discover(None, root.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "PDF-Processing");
    }

    #[test]
    fn disable_model_invocation_is_recorded() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".tau/skills/s",
            "---\nname: s\ndescription: d\ndisable-model-invocation: true\n---\nBody.\n",
        );
        let skills = discover(None, root.path());
        assert!(!skills[0].model_invocation);
        assert!(catalog(&skills).is_none());
    }

    #[test]
    fn expand_matches_the_template_with_and_without_args() {
        let dir = Path::new("/proj/.agents/skills/my-skill");
        let body = "Step one.\nStep two.";
        assert_eq!(
            expand("my-skill", body, dir, None),
            "Skill `my-skill` — follow the instructions below. The skill directory is /proj/.agents/skills/my-skill; resolve relative paths in the instructions against it.\n\nStep one.\nStep two."
        );
        assert_eq!(
            expand("my-skill", body, dir, Some("  do the thing  ")),
            "Skill `my-skill` — follow the instructions below. The skill directory is /proj/.agents/skills/my-skill; resolve relative paths in the instructions against it.\n\nStep one.\nStep two.\n\nUser request: do the thing"
        );
    }

    #[test]
    fn the_catalog_lists_names_descriptions_and_locations() {
        let skills = vec![
            Skill {
                name: "a".into(),
                description: "da".into(),
                location: PathBuf::from("/sys/skills/a/SKILL.md"),
                model_invocation: true,
            },
            Skill {
                name: "b".into(),
                description: "db".into(),
                location: PathBuf::from("/proj/.agents/skills/b/SKILL.md"),
                model_invocation: false,
            },
        ];
        let cat = catalog(&skills).unwrap();
        assert!(cat.starts_with("The following skills provide specialized instructions"));
        assert!(cat.contains("<name>a</name>"));
        assert!(cat.contains("<location>/sys/skills/a/SKILL.md</location>"));
        assert!(!cat.contains(">b<"));
        assert!(catalog(&[]).is_none());
        assert!(catalog(&[skills[1].clone()]).is_none());
    }

    #[test]
    fn body_strips_the_frontmatter() {
        assert_eq!(
            body("---\nname: s\ndescription: d\n---\n\nBody text.\n"),
            "Body text."
        );
    }
}
