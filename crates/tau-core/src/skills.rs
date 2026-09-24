//! Skills (the spec's "Not in v0" entry; ticket #28): the Agent Skills
//! standard — a directory containing `SKILL.md` (YAML frontmatter + a
//! markdown body). Discovery is per workspace, project wins: the system
//! `~/.config/tau/skills/` and the home's cross-client `~/.agents/skills/`,
//! then the project's `.tau/skills/` and the cross-client `.agents/skills/`
//! convention. Validation is lenient per
//! the integrate guide: a missing description or unparseable frontmatter
//! skips the skill, a name violation only warns. A skill with
//! `disable-model-invocation: true` is excluded from the catalog — the
//! user's `/skill:` invocation is its only door.

use std::collections::BTreeMap;
use std::ffi::OsStr;
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

/// Discover the workspace's registry: the system scope first (the config
/// dir's `skills/`, then the home's cross-client `.agents/skills/`), then
/// the project's (its `.tau/skills/`, then the cross-client
/// `.agents/skills/`). Project wins over system; within a scope the
/// first in the sorted scan wins and the shadow is logged.
pub fn discover(
    system_dir: Option<&Path>,
    user_home: Option<&Path>,
    project_root: &Path,
) -> Vec<Skill> {
    // Scope rank for collisions: project (1) beats system (0); within a
    // scope the first in the sorted scan wins — the shadow is logged.
    let mut reg: BTreeMap<String, (usize, Skill)> = BTreeMap::new();
    for root in [
        system_dir.map(|d| d.join("skills")),
        user_home.map(|h| h.join(".agents").join("skills")),
    ]
    .into_iter()
    .flatten()
    {
        for s in scan(&root) {
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
        if let Some(skill_md) = skill_md_in(&d)
            && let Ok(raw) = std::fs::read_to_string(&skill_md)
            && let Some(s) = parse(&raw, &skill_md)
        {
            out.push(s);
            continue;
        }
        walk(&d, depth + 1, out);
    }
}

/// The `SKILL.md` in `dir`, matched against the enumerated entry name
/// byte-for-byte. A `join("SKILL.md")` + `is_file()` check would accept
/// `skill.md` on a case-insensitive filesystem (the macOS default), which
/// the standard's "named exactly `SKILL.md`" excludes.
fn skill_md_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        if e.file_name() == OsStr::new("SKILL.md") && e.path().is_file() {
            return Some(e.path());
        }
    }
    None
}

/// A `SKILL.md`: `---` frontmatter over the body. Lenient per the
/// integrate guide: unparseable frontmatter skips the skill; a missing
/// description skips it; a name violation (uppercase, over 64, …) or a
/// description over the standard's 1024-char cap only warns.
fn parse(raw: &str, location: &Path) -> Option<Skill> {
    let Some(fields) = frontmatter_fields(raw) else {
        eprintln!("{}", skip_message(location, "unparseable frontmatter"));
        return None;
    };
    let Some(name) = fields.get("name").cloned().filter(|n| !n.is_empty()) else {
        eprintln!(
            "{}",
            skip_message(location, "missing required field 'name'")
        );
        return None;
    };
    let description = fields.get("description").cloned().unwrap_or_default();
    if description.is_empty() {
        eprintln!(
            "{}",
            skip_message(location, "missing required field 'description'")
        );
        return None;
    }
    if !valid_name(&name) {
        eprintln!("{}", name_violation_message(location, &name));
    }
    if description.chars().count() > 1024 {
        eprintln!("{}", description_violation_message(location));
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

/// A skip diagnostic: the offending file's path, then the specific cause.
fn skip_message(location: &Path, detail: &str) -> String {
    format!("{}: {detail} — skipped", location.display())
}

/// A name violation: the file's path and the offending name; the skill is
/// loaded anyway, per the lenient rule.
fn name_violation_message(location: &Path, name: &str) -> String {
    format!(
        "{}: name '{name}' violates [a-z0-9-] (loaded anyway)",
        location.display()
    )
}

/// A description over the standard's 1024-char cap: the file's path; the
/// skill is loaded anyway, per the lenient posture.
fn description_violation_message(location: &Path) -> String {
    format!(
        "{}: description exceeds 1024 chars (loaded anyway)",
        location.display()
    )
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
mod tests;
