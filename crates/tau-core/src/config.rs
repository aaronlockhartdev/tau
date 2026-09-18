//! Merged configuration: system `~/.config/tau/config.toml` with the project
//! `.tau/config.toml` layered on top (spec §12). TOML was chosen as the format
//! at the scaffold milestone — map ticket #16, resolving spec U2.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

/// A named OpenAI-compatible provider entry: base URL, env var holding the key,
/// and the model ids to offer (spec §6: no keychain, no auto-detection).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    /// API root, e.g. `http://127.0.0.1:1234/v1`.
    pub base_url: String,
    /// Name of the environment variable holding the API key; empty = no key.
    pub key_env: String,
    pub models: Vec<String>,
}

/// Observational Memory settings; `om_model` is global, not per-provider (spec §4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Om {
    pub om_model: String,
    pub observe_threshold: u64,
    pub reflect_threshold: u64,
    pub buffer_increment: u64,
}

impl Default for Om {
    fn default() -> Self {
        Self {
            om_model: String::new(),
            observe_threshold: 30_000,
            reflect_threshold: 40_000,
            buffer_increment: 6_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubAgents {
    pub max_depth: u32,
    /// Concurrent children; unset means unlimited.
    pub max_concurrent: Option<u32>,
}

impl Default for SubAgents {
    fn default() -> Self {
        Self {
            max_depth: 1,
            max_concurrent: None,
        }
    }
}

/// What a force does to an in-flight tool batch (spec §7: complete | kill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolBatchPolicy {
    #[default]
    Complete,
    Kill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Requests {
    pub timeout_secs: u64,
    pub retries: u32,
    pub tool_batch_on_force: ToolBatchPolicy,
}

impl Default for Requests {
    fn default() -> Self {
        Self {
            timeout_secs: 120,
            retries: 2,
            tool_batch_on_force: ToolBatchPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gui {
    pub coalesce_ms: u64,
    pub reasoning_visible: bool,
}

impl Default for Gui {
    fn default() -> Self {
        Self {
            coalesce_ms: 25,
            reasoning_visible: true,
        }
    }
}

/// Session storage settings (spec §3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sessions {
    pub blob_threshold_bytes: u64,
}

impl Default for Sessions {
    fn default() -> Self {
        Self {
            blob_threshold_bytes: 100_000,
        }
    }
}

/// The merged, fully-defaulted configuration the core operates on.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub providers: BTreeMap<String, Provider>,
    pub om: Om,
    pub subagents: SubAgents,
    pub requests: Requests,
    pub gui: Gui,
    pub sessions: Sessions,
}

/// On-disk file shape: every section optional so a partial file layers cleanly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    providers: Option<BTreeMap<String, Provider>>,
    om: Option<Om>,
    subagents: Option<SubAgents>,
    requests: Option<Requests>,
    gui: Option<Gui>,
    sessions: Option<Sessions>,
}

impl ConfigFile {
    fn read(path: &Path) -> Result<Self, LoadError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(LoadError::Io(e)),
        };
        toml::from_str(&text).map_err(|e| LoadError::Parse(path.to_path_buf(), e))
    }

    fn merge_into(&self, config: &mut Config) {
        if let Some(providers) = &self.providers {
            config.providers = providers.clone();
        }
        if let Some(om) = &self.om {
            config.om = om.clone();
        }
        if let Some(subagents) = &self.subagents {
            config.subagents = subagents.clone();
        }
        if let Some(requests) = &self.requests {
            config.requests = requests.clone();
        }
        if let Some(gui) = &self.gui {
            config.gui = gui.clone();
        }
        if let Some(sessions) = &self.sessions {
            config.sessions = sessions.clone();
        }
    }
}

/// Load the merged config from the system directory and the project directory.
/// Sections layer over the system values; a project provider entry of the same
/// name replaces the system entry wholesale.
pub fn load(system_dir: &Path, project_dir: &Path) -> Result<Config, LoadError> {
    let system = ConfigFile::read(&system_dir.join("config.toml"))?;
    let project = ConfigFile::read(&project_dir.join("config.toml"))?;
    let mut config = Config::default();
    system.merge_into(&mut config);
    project.merge_into(&mut config);
    Ok(config)
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Parse(std::path::PathBuf, toml::de::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading config: {e}"),
            Self::Parse(path, e) => write!(f, "parsing {}: {e}", path.display()),
        }
    }
}

impl Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_from(system: &str, project: &str) -> Result<Config, LoadError> {
        let (sys_dir, proj_dir) = (tempfile::tempdir()?, tempfile::tempdir()?);
        if !system.is_empty() {
            std::fs::write(sys_dir.path().join("config.toml"), system)?;
        }
        if !project.is_empty() {
            std::fs::write(proj_dir.path().join("config.toml"), project)?;
        }
        load(sys_dir.path(), proj_dir.path())
    }

    #[test]
    fn absent_files_yield_defaults() {
        let config = load_from("", "").unwrap();
        assert!(config.providers.is_empty());
        assert_eq!(config.om.observe_threshold, 30_000);
        assert_eq!(config.om.reflect_threshold, 40_000);
        assert_eq!(config.om.buffer_increment, 6_000);
        assert_eq!(config.subagents.max_depth, 1);
        assert_eq!(config.gui.coalesce_ms, 25);
        assert_eq!(config.sessions.blob_threshold_bytes, 100_000);
        assert!(config.gui.reasoning_visible);
    }

    #[test]
    fn project_layer_overrides_system_layer() {
        let system = r#"
[providers.local]
base_url = "http://system:1/v1"
key_env = "SYS_KEY"
models = ["sys-model"]

[gui]
coalesce_ms = 50
"#;
        let project = r#"
[providers.local]
base_url = "http://project:2/v1"

[gui]
coalesce_ms = 25
"#;
        let config = load_from(system, project).unwrap();
        let local = &config.providers["local"];
        // Provider entries replace wholesale — a project entry is a unit, so
        // omitted fields fall back to their own defaults, not to system values.
        assert_eq!(local.base_url, "http://project:2/v1");
        assert_eq!(local.key_env, "");
        assert_eq!(config.gui.coalesce_ms, 25);
    }

    #[test]
    fn partial_sections_fill_their_own_defaults() {
        let config = load_from("", "[om]\nom_model = \"m1\"\n").unwrap();
        assert_eq!(config.om.om_model, "m1");
        assert_eq!(config.om.observe_threshold, 30_000);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = load_from("", "[om]\nbogus = 1\n").unwrap_err();
        match err {
            LoadError::Parse(path, _) => assert!(path.ends_with("config.toml")),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }
}
