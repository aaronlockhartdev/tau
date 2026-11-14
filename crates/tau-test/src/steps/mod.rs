//! Step definitions: the vocabulary a scenario may use, and the source
//! `bin steps` generates `STEPS.md` from.

use crate::world::World;
use cucumber::{given, then, when};
use std::fs;
use std::process::Command;

/// The `[package] name = "…"` value in a Cargo.toml, without a TOML dependency.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && line
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == "name")
        {
            return line
                .split_once('=')?
                .1
                .trim()
                .trim_matches('"')
                .to_string()
                .into();
        }
    }
    None
}

fn report(world: &mut World, finding: impl Into<String>) {
    world.findings.push(finding.into());
}

fn assert_clean(world: &mut World, expectation: &str) {
    if !world.findings.is_empty() {
        panic!("{expectation}: {}", world.findings.join("; "));
    }
}

#[given("the workspace manifest")]
async fn workspace_manifest(world: &mut World) {
    world.root = World::repo_root();
    world.findings.clear();
    if !world.root.join("Cargo.toml").is_file() {
        report(world, "no root Cargo.toml".to_string());
    }
}

#[when("the members are listed")]
async fn members_are_listed(world: &mut World) {
    let text = fs::read_to_string(world.root.join("Cargo.toml")).unwrap_or_default();
    if !text.contains("members = [\"crates/*\"]") {
        report(world, "members is not the crates/* glob".to_string());
    }
    if !text.contains("resolver = \"3\"") {
        report(world, "resolver is not 3".to_string());
    }
    if text
        .lines()
        .any(|l| l.trim_start().starts_with("exclude") && l.contains('='))
    {
        report(
            world,
            "the members list is hand-maintained by an exclude".to_string(),
        );
    }
}

#[then("the members come from the crates glob")]
async fn members_from_glob(world: &mut World) {
    assert_clean(world, "the members come from the crates glob");
}

#[when("the crates are read")]
async fn crates_are_read(world: &mut World) {
    let crates = world.root.join("crates");
    let Ok(entries) = fs::read_dir(crates) else {
        report(world, "no crates directory".to_string());
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // The manifest is looked for as a file, so a crate directory without one
        // is a finding, not a crate skipped in silence.
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            report(world, format!("{} has no Cargo.toml", dir.display()));
            continue;
        }
        let Ok(text) = fs::read_to_string(&manifest) else {
            report(world, format!("{} is unreadable", manifest.display()));
            continue;
        };
        match package_name(&text) {
            Some(got) if got == name => {}
            Some(got) => report(world, format!("{} names the package {got}", dir.display())),
            None => report(world, format!("{} declares no package name", dir.display())),
        }
    }
}

#[then("every crate is named after its directory")]
async fn named_after_directory(world: &mut World) {
    assert_clean(world, "every crate is named after its directory");
}

#[given("the git index")]
async fn git_index(world: &mut World) {
    world.root = World::repo_root();
    world.tracked.clear();
    world.findings.clear();
}

#[when("the local state paths are listed")]
async fn local_state_listed(world: &mut World) {
    let out = Command::new("git")
        .args(["ls-files", "target", ".tau"])
        .current_dir(&world.root)
        .output();
    world.tracked = match out {
        Ok(ok) if ok.status.success() => String::from_utf8_lossy(&ok.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        Ok(ok) => {
            report(
                world,
                format!("git ls-files: {}", String::from_utf8_lossy(&ok.stderr)),
            );
            Vec::new()
        }
        Err(e) => {
            report(world, format!("git ls-files: {e}"));
            Vec::new()
        }
    };
}

#[then("target and .tau are untracked")]
async fn local_state_untracked(world: &mut World) {
    // Findings from the listing step (a failed git run) are reported here too.
    for path in &world.tracked {
        world.findings.push(format!("{path} is tracked in git"));
    }
    if !world.findings.is_empty() {
        panic!(
            "target and .tau are untracked: {}",
            world.findings.join("; ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::package_name;

    #[test]
    fn package_name_comes_from_the_package_table() {
        assert_eq!(
            package_name("[package]\nname = \"tau-core\"\nversion = \"0.0.0\"\n"),
            Some("tau-core".to_string())
        );
        // A dependency's name lives in a [dependencies] table, not in [package].
        assert_eq!(package_name("[dependencies]\nname = \"tokio\"\n"), None);
        assert_eq!(package_name("[package]\nversion = \"0.0.0\"\n"), None);
    }
}
