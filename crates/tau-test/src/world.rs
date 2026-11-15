use cucumber::World as CucumberWorld;
use std::path::PathBuf;
use tau_core::storage::Store;

/// The lane's state for one scenario: the repo root the steps read, what a step
/// found wrong, and the git index rows a step listed.
#[derive(Debug, Default, CucumberWorld)]
pub struct World {
    pub root: PathBuf,
    pub findings: Vec<String>,
    pub tracked: Vec<String>,
    /// The store the scenario opened, and the file it opened it at.
    pub store: Option<Store>,
    pub store_path: PathBuf,
    /// The project tau opened, and the checkout root it was given.
    pub project: Option<tau_core::project::Project>,
    pub project_root: PathBuf,
    /// The bytes an export step wrote, so the next step can read the lines back.
    pub exported: String,
    /// What the setup child said and how it ended, so the next step can read the
    /// caller-visible refusal back (R-session-33).
    pub setup_child: String,
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
