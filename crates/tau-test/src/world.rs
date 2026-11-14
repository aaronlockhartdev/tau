use cucumber::World as CucumberWorld;
use std::path::PathBuf;

/// The lane's state for one scenario: the repo root the steps read, what a step
/// found wrong, and the git index rows a step listed.
#[derive(Debug, Default, CucumberWorld)]
pub struct World {
    pub root: PathBuf,
    pub findings: Vec<String>,
    pub tracked: Vec<String>,
}

impl World {
    /// The repo root, above the crate directory the test target runs from.
    pub fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|crates| crates.parent())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}
