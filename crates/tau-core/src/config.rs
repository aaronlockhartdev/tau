//! Merged configuration: system `~/.config/tau/config.toml` with the project
//! `.tau/config.toml` layered on top (spec §12). TOML was chosen as the format
//! at the scaffold milestone — map ticket #16, resolving spec U2.
//!
//! The settled key layout (spec §12, #35): `providers` (connection),
//! `generation` (sampling), `thinking` (reasoning effort), `cache`
//! (prompt-cache policy), `requests` (resilience), `om` (memory),
//! `subagents` (delegation), `limits` (size / safety).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

/// A named OpenAI-compatible provider entry: base URL, env var holding the key,
/// and the models it offers (spec §6: no keychain, no auto-detection).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    /// API root, e.g. `http://127.0.0.1:1234/v1`.
    pub base_url: String,
    /// Name of the environment variable holding the API key; empty = no key.
    pub key_env: String,
    /// The offered models: id = key, facts optional (#35).
    pub models: BTreeMap<String, ModelDef>,
}

impl Provider {
    /// A single-model entry with no facts (the common test shape).
    pub fn with_model(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        let mut models = BTreeMap::new();
        models.insert(model.into(), ModelDef::default());
        Self {
            base_url: base_url.into(),
            key_env: String::new(),
            models,
        }
    }
}

/// The user-authored facts of one model, inline under its provider (spec
/// §12, #35): no separate catalog file — fidelity is the user's
/// responsibility (§6). Every fact is optional; absent = sane default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelDef {
    /// Total context window in tokens; when declared, the output cap is
    /// clamped to `context_window` − prompt.
    pub context_window: Option<u32>,
    /// This model's output cap.
    pub max_tokens: Option<u32>,
    /// The model reasons (thinking levels are offered for it).
    pub reasoning: Option<bool>,
    /// Per-1M-token cost (display fact; the GUI's cost display is later).
    pub cost: Option<Cost>,
}

/// Per-1M-token cost (USD).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
}

/// Sampling defaults (spec §12, #35); `None` = the server's own default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Generation {
    /// The session's model; empty = the provider's first model.
    pub default_model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
}

/// A thinking level (spec §12, #35).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

/// Reasoning-effort policy (spec §12, #35).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Thinking {
    /// The default level for the session's model.
    pub level: ThinkingLevel,
    /// Per-model level overrides (model id → level).
    pub levels: BTreeMap<String, ThinkingLevel>,
    /// Per-level token budgets (level name → tokens).
    pub budgets: BTreeMap<String, u32>,
    /// Ask for a reasoning summary (`reasoning.summary`).
    pub summary: bool,
}

/// Prompt-cache retention (spec §12, #35): `short` = 24 h, `long` = 7 d.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    #[default]
    None,
    Short,
    Long,
}

/// Prompt-cache warming (spec §12, #35).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarming {
    #[default]
    Off,
    Streaming,
    Idle,
}

/// Prompt-cache policy (spec §12, #35).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cache {
    pub retention: CacheRetention,
    pub warming: CacheWarming,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Requests {
    /// The connect/headers phase's deadline (seconds); the stream body is
    /// bounded separately by `idle_timeout_secs`.
    pub timeout_secs: u64,
    pub retries: u32,
    /// The stream's idle timeout (seconds): a stream silent for this long
    /// (connect, headers, or between chunks) is dead. Per-chunk — reset on
    /// every chunk — never a total deadline (a total killed healthy long
    /// streams mid-body).
    pub idle_timeout_secs: u64,
    pub tool_batch_on_force: ToolBatchPolicy,
}

impl Default for Requests {
    fn default() -> Self {
        Self {
            timeout_secs: 120,
            retries: 2,
            idle_timeout_secs: 120,
            tool_batch_on_force: ToolBatchPolicy::default(),
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

/// Image-read caps (spec §12, #35): the home for #34's enforcement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_bytes: u64,
    /// 1–100, when a read transcodes to JPEG.
    pub jpeg_quality: u8,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_width: 2048,
            max_height: 2048,
            max_bytes: 10 * 1024 * 1024,
            jpeg_quality: 80,
        }
    }
}

/// Size / safety caps (spec §12, #35).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub image: ImageLimits,
    /// Total request-body cap in bytes; unset = unbounded.
    pub max_request_bytes: Option<u64>,
}

/// The merged, fully-defaulted configuration the core operates on.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub providers: BTreeMap<String, Provider>,
    pub generation: Generation,
    pub thinking: Thinking,
    pub cache: Cache,
    pub requests: Requests,
    pub om: Om,
    pub subagents: SubAgents,
    pub limits: Limits,
}

/// On-disk file shape: every section optional so a partial file layers cleanly.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    providers: Option<BTreeMap<String, Provider>>,
    generation: Option<Generation>,
    thinking: Option<Thinking>,
    cache: Option<Cache>,
    requests: Option<Requests>,
    om: Option<Om>,
    subagents: Option<SubAgents>,
    limits: Option<Limits>,
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
        if let Some(generation) = &self.generation {
            config.generation = generation.clone();
        }
        if let Some(thinking) = &self.thinking {
            config.thinking = thinking.clone();
        }
        if let Some(cache) = &self.cache {
            config.cache = cache.clone();
        }
        if let Some(requests) = &self.requests {
            config.requests = requests.clone();
        }
        if let Some(om) = &self.om {
            config.om = om.clone();
        }
        if let Some(subagents) = &self.subagents {
            config.subagents = subagents.clone();
        }
        if let Some(limits) = &self.limits {
            config.limits = limits.clone();
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
mod tests;
