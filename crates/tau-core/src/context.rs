//! Context-file discovery and system-prompt assembly (spec §10): pi's
//! pattern — `AGENTS.md` or `CLAUDE.md` from the global agent directory,
//! from each parent directory walking up from the workspace cwd, and from
//! the cwd itself; a per-directory `AGENTS.override.md` replaces that
//! directory's `AGENTS.md`/`CLAUDE.md`; all layers appended, nearest last.

use std::path::{Path, PathBuf};

const FILENAMES: [&str; 3] = ["AGENTS.override.md", "AGENTS.md", "CLAUDE.md"];

/// One discovered context-file layer, in assembly order (global first, the
/// cwd last — nearest to the generation point).
#[derive(Debug, Clone)]
pub struct Layer {
    pub path: PathBuf,
    pub text: String,
}

/// Discover the context-file layers for `cwd` (the workspace root).
pub fn discover(cwd: &Path, global: &Path) -> Vec<Layer> {
    let mut layers = Vec::new();
    push_layer(&mut layers, global);

    // The walk-up chain from cwd to the filesystem root, reversed so the
    // directories are ordered root-first (global, then far to near, cwd last).
    let mut chain: Vec<PathBuf> = vec![cwd.to_path_buf()];
    let mut dir = cwd.parent().map(|p| p.to_path_buf());
    while let Some(parent) = dir {
        chain.push(parent.clone());
        dir = parent.parent().map(|p| p.to_path_buf());
    }
    chain.reverse();
    for dir in chain {
        push_layer(&mut layers, &dir);
    }
    layers
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
/// (the type's prompt is never inherited from a parent — spec §10).
pub fn assemble(base: &str, layers: &[Layer]) -> String {
    if layers.is_empty() {
        return base.to_owned();
    }
    let mut out = String::from(base);
    for layer in layers {
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
        let prompt = assemble("base", &layers);
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
        assert_eq!(assemble("base", &layers), "base");
    }
}
