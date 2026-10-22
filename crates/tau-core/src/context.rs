//! Context-file discovery and system-prompt assembly (spec §10): pi's
//! pattern — `AGENTS.md` or `CLAUDE.md` from the global agent directory,
//! from each parent directory walking up from the workspace cwd, and from
//! the cwd itself; a per-directory `AGENTS.override.md` replaces that
//! directory's `AGENTS.md`/`CLAUDE.md`; all layers appended, nearest last.

use std::path::{Path, PathBuf};

const FILENAMES: [&str; 3] = ["AGENTS.override.md", "AGENTS.md", "CLAUDE.md"];

/// One discovered context-file layer, in assembly order (global first, the
/// cwd last — nearest to the generation point).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub path: PathBuf,
    pub text: String,
}

/// Discover the context-file layers for `cwd` (the workspace root).
#[must_use]
pub fn discover(cwd: &Path, global: &Path) -> Vec<Layer> {
    let mut layers = Vec::new();
    push_layer(&mut layers, global);

    // The walk-up chain from cwd to the filesystem root, reversed so the
    // directories are ordered root-first (global, then far to near, cwd last).
    let mut chain: Vec<PathBuf> = vec![cwd.to_path_buf()];
    let mut dir = cwd.parent().map(std::path::Path::to_path_buf);
    while let Some(parent) = dir {
        chain.push(parent.clone());
        dir = parent.parent().map(std::path::Path::to_path_buf);
    }
    chain.reverse();
    for dir in chain {
        push_layer(&mut layers, &dir);
    }
    layers
}
/// The resolved pi prompt files (ticket #77): `system` replaces the base
/// prompt, `append` composes on top of whatever base wins.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptFiles {
    pub system: Option<String>,
    pub append: Option<Layer>,
}

/// Resolve the pi prompt files for `cwd` (ticket #77): the project's
/// `<cwd>/.pi/` before the user's `<home>/.pi/`; the first hit per type
/// wins and the two types resolve independently. Missing files (or no home)
/// leave the field `None`.
#[must_use]
pub fn resolve_prompt_files(cwd: &Path, home: Option<&Path>) -> PromptFiles {
    let project = cwd.join(".pi");
    let global = home.map(|h| h.join(".pi"));
    // Project first: a hit there ends the lookup for that type.
    let system = read_optional(&project.join("SYSTEM.md"))
        .or_else(|| {
            global
                .as_ref()
                .and_then(|g| read_optional(&g.join("SYSTEM.md")))
        })
        .map(|l| l.text);
    let append = read_optional(&project.join("APPEND_SYSTEM.md")).or_else(|| {
        global
            .as_ref()
            .and_then(|g| read_optional(&g.join("APPEND_SYSTEM.md")))
    });
    PromptFiles { system, append }
}

/// A readable file becomes a layer: the path doubles as the section heading
/// in `assemble`.
fn read_optional(path: &Path) -> Option<Layer> {
    if path.is_file() {
        std::fs::read_to_string(path).ok().map(|text| Layer {
            path: path.to_path_buf(),
            text,
        })
    } else {
        None
    }
}

fn push_layer(layers: &mut Vec<Layer>, dir: &Path) {
    for name in FILENAMES {
        let path = dir.join(name);
        if path.is_file() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                layers.push(Layer { path, text });
            }
            return; // the first present file for this directory wins
        }
    }
}

/// Assemble the system prompt: the agent type's base prompt, then each layer
/// (the type's prompt is never inherited from a parent — spec §10), then the
/// pi `APPEND_SYSTEM.md` section (ticket #77) after the context layers.
#[must_use]
pub fn assemble(base: &str, layers: &[Layer], append: Option<&Layer>) -> String {
    let mut out = String::from(base);
    for layer in layers.iter().chain(append.iter().copied()) {
        out.push_str("\n\n# ");
        out.push_str(&layer.path.display().to_string());
        out.push('\n');
        out.push_str(&layer.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_up_and_appends_far_to_near() {
        let root = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        let sub = root.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(global.path().join("AGENTS.md"), "global").unwrap();
        std::fs::write(root.path().join("CLAUDE.md"), "root").unwrap();
        std::fs::write(root.path().join("a").join("AGENTS.md"), "mid").unwrap();
        std::fs::write(sub.join("AGENTS.md"), "near").unwrap();

        let layers = discover(&sub, global.path());
        let names: Vec<String> = layers
            .iter()
            .map(|l| l.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["AGENTS.md", "CLAUDE.md", "AGENTS.md", "AGENTS.md"]
        );
        let prompt = assemble("base", &layers, None);
        assert!(prompt.starts_with("base"));
        assert!(prompt.find("global").unwrap() < prompt.find("root").unwrap());
        assert!(prompt.find("root").unwrap() < prompt.find("mid").unwrap());
        assert!(prompt.find("mid").unwrap() < prompt.find("near").unwrap());
    }

    #[test]
    fn override_replaces_that_directory_only() {
        let root = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "normal").unwrap();
        std::fs::write(root.path().join("AGENTS.override.md"), "override").unwrap();
        let layers = discover(root.path(), global.path());
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].text, "override");
    }

    #[test]
    fn missing_layers_yield_the_base_prompt_alone() {
        let global = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let layers = discover(cwd.path(), global.path());
        assert!(layers.is_empty());
        assert_eq!(assemble("base", &layers, None), "base");
    }

    #[test]
    fn project_system_md_wins_over_global() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".pi")).unwrap();
        std::fs::create_dir_all(home.path().join(".pi")).unwrap();
        std::fs::write(cwd.path().join(".pi").join("SYSTEM.md"), "project").unwrap();
        std::fs::write(home.path().join(".pi").join("SYSTEM.md"), "global").unwrap();

        let files = resolve_prompt_files(cwd.path(), Some(home.path()));
        assert_eq!(files.system.as_deref(), Some("project"));
        assert!(files.append.is_none());
    }

    #[test]
    fn global_system_md_is_the_fallback() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".pi")).unwrap();
        std::fs::write(home.path().join(".pi").join("SYSTEM.md"), "global").unwrap();

        let files = resolve_prompt_files(cwd.path(), Some(home.path()));
        assert_eq!(files.system.as_deref(), Some("global"));
    }

    #[test]
    fn append_is_resolved_independently_of_system() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cwd.path().join(".pi")).unwrap();
        std::fs::create_dir_all(home.path().join(".pi")).unwrap();
        std::fs::write(cwd.path().join(".pi").join("SYSTEM.md"), "project-sys").unwrap();
        // Project APPEND wins over the global one — per type, project first.
        std::fs::write(
            cwd.path().join(".pi").join("APPEND_SYSTEM.md"),
            "project-append",
        )
        .unwrap();
        std::fs::write(
            home.path().join(".pi").join("APPEND_SYSTEM.md"),
            "global-append",
        )
        .unwrap();

        let files = resolve_prompt_files(cwd.path(), Some(home.path()));
        assert_eq!(files.system.as_deref(), Some("project-sys"));
        let append = files.append.as_ref().expect("an append file");
        assert_eq!(append.text, "project-append");
        // The layer carries the real path: the section is labeled by location.
        assert!(append.path.ends_with("APPEND_SYSTEM.md"));
    }

    #[test]
    fn no_prompt_files_yield_none_for_both() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let files = resolve_prompt_files(cwd.path(), Some(home.path()));
        assert_eq!(files.system, None);
        assert_eq!(files.append, None);
        // No home (the bare test kits): the global leg is simply skipped.
        let files = resolve_prompt_files(cwd.path(), None);
        assert_eq!(files.system, None);
        assert_eq!(files.append, None);
    }

    #[test]
    fn append_composes_after_the_layers() {
        let layer = Layer {
            path: "/proj/AGENTS.md".into(),
            text: "agents".into(),
        };
        let append = Layer {
            path: "/proj/.pi/APPEND_SYSTEM.md".into(),
            text: "extra rules".into(),
        };
        let prompt = assemble("base", std::slice::from_ref(&layer), Some(&append));
        assert!(prompt.starts_with("base"));
        // Order: base, context layers, then the append section.
        assert!(
            prompt.find("agents").unwrap() < prompt.find("extra rules").unwrap(),
            "the append section lands after the context layers: {prompt}"
        );
        // The section is labeled with the file's path, like a layer.
        assert!(prompt.contains("# /proj/.pi/APPEND_SYSTEM.md"), "{prompt}");
    }
}
