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
    let skills = discover(Some(&system), None, &project);
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
    let skills = discover(None, None, root.path());
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
    let skills = discover(None, None, root.path());
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["deep-ok"]);
}

#[test]
fn missing_description_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    write_skill(root.path(), ".tau/skills/s", "---\nname: s\n---\nBody.\n");
    assert!(discover(None, None, root.path()).is_empty());
}

#[test]
fn unparseable_frontmatter_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    write_skill(
        root.path(),
        ".tau/skills/s",
        "---\nname s\ndescription: d\n---\nBody.\n",
    );
    assert!(discover(None, None, root.path()).is_empty());
}

#[test]
fn an_unquoted_colon_value_loads_via_the_quoted_retry() {
    let root = tempfile::tempdir().unwrap();
    write_skill(
        root.path(),
        ".tau/skills/s",
        "---\nname: s\ndescription: Use this when: the user asks for it\n---\nBody.\n",
    );
    let skills = discover(None, None, root.path());
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
    let skills = discover(None, None, root.path());
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
    let skills = discover(None, None, root.path());
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

#[test]
fn a_non_exact_case_skill_md_is_not_discovered() {
    let root = tempfile::tempdir().unwrap();
    // Named `skill.md`, not the standard's `SKILL.md`; written by name so
    // a case-insensitive filesystem cannot paper over the mismatch.
    let dir = root.path().join(".agents").join("skills").join("lower");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("skill.md"), skill("lower", "d")).unwrap();
    // A correctly-cased sibling is still discovered, so the scan itself
    // is proven to work.
    write_skill(root.path(), ".agents/skills/ok", &skill("ok", "d"));
    let skills = discover(None, None, root.path());
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["ok"]);
}

#[test]
fn diagnostics_carry_the_path_and_the_failed_field() {
    let p = Path::new("/proj/.agents/skills/broken/SKILL.md");
    assert_eq!(
        skip_message(p, "missing required field 'description'"),
        "/proj/.agents/skills/broken/SKILL.md: missing required field 'description' — skipped"
    );
    assert_eq!(
        skip_message(p, "unparseable frontmatter"),
        "/proj/.agents/skills/broken/SKILL.md: unparseable frontmatter — skipped"
    );
    assert_eq!(
        name_violation_message(p, "My-Skill"),
        "/proj/.agents/skills/broken/SKILL.md: name 'My-Skill' violates [a-z0-9-] (loaded anyway)"
    );
    assert_eq!(
        description_violation_message(p),
        "/proj/.agents/skills/broken/SKILL.md: description exceeds 1024 chars (loaded anyway)"
    );
}

#[test]
fn a_user_level_agents_skill_is_discovered() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let project = root.path().join("project");
    write_skill(
        &home,
        ".agents/skills/user",
        &skill("user", "from the home .agents"),
    );
    let skills = discover(None, Some(&home), &project);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description, "from the home .agents");
}

#[test]
fn a_project_agents_skill_shadows_the_user_level_one() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let project = root.path().join("project");
    write_skill(
        &home,
        ".agents/skills/shared",
        &skill("shared", "from the home .agents"),
    );
    write_skill(
        &project,
        ".agents/skills/shared",
        &skill("shared", "from the project .agents"),
    );
    let skills = discover(None, Some(&home), &project);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description, "from the project .agents");
}

#[test]
fn a_description_over_the_cap_warns_but_loads() {
    let root = tempfile::tempdir().unwrap();
    let long = "d".repeat(1025);
    write_skill(
        root.path(),
        ".tau/skills/s",
        &format!("---\nname: s\ndescription: {long}\n---\nBody.\n"),
    );
    let skills = discover(None, None, root.path());
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description.chars().count(), 1025);
}
