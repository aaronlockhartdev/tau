//! The TOML sections of one config layer (spec §12): the structs that parse
//! from the file, each with its defaults and the validation the load runs on
//! it. `Config` merges these; `LoadError` is the load's failure list.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::LoadError;
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
    /// The context window a model entry carries when nothing declares one: a
    /// model with no window has no gate, and #338 made the window a required
    /// fact, so an entry built in code (the ACP env channel, a test kit)
    /// declares this pair rather than leaving the prompt unbounded.
    pub const DEFAULT_WINDOW: u32 = 131_072;
    /// The output cap that pairs with [`Provider::DEFAULT_WINDOW`], comfortably
    /// under it so the gate's margin always fits.
    pub const DEFAULT_MAX_OUTPUT: u32 = 16_384;

    /// The model entry a code-built provider carries when the channel that
    /// built it names no facts (the ACP env override, a `_meta` install): the
    /// pair above, because #338 requires a window and a gate-less model would
    /// send unbounded prompts.
    #[must_use]
    pub const fn default_def() -> ModelDef {
        ModelDef {
            context_window: Self::DEFAULT_WINDOW,
            max_tokens: Some(Self::DEFAULT_MAX_OUTPUT),
            reasoning: None,
            cost: None,
            generation: None,
        }
    }

    /// A single-model entry with the two gate facts (the common test shape);
    /// a test that needs other facts sets the model def itself.
    pub fn with_model(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        let mut models = BTreeMap::new();
        models.insert(model.into(), Self::default_def());
        Self {
            base_url: base_url.into(),
            key_env: String::new(),
            models,
        }
    }
}

/// The user-authored facts of one model, inline under its provider (spec
/// §12, #35): no separate catalog file — fidelity is the user's
/// responsibility (§6). `context_window` is required (#338); the rest are
/// optional, absent = sane default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelDef {
    /// Total context window in tokens, the gate's bound: the output cap
    /// derives from it as `min(max_tokens, window − prompt − margin)`. A model
    /// that declares none has no gate, so a config that omits it (or writes 0)
    /// fails to load — an unbounded prompt is the failure #338 refuses.
    /// `0` is the undeclared sentinel; no real model ships a 0-token window.
    #[serde(default)]
    pub context_window: u32,
    /// This model's output cap — a hard ceiling on the answer, which the window
    /// shrinks per call; it never reserves prompt room (#338, #343).
    pub max_tokens: Option<u32>,
    /// The model reasons (thinking levels are offered for it).
    pub reasoning: Option<bool>,
    /// Per-1M-token cost (display fact; the GUI's cost display is later).
    pub cost: Option<Cost>,
    /// The per-model generation profile (#407): what mastra calls
    /// modelSettings for this model. It reaches this model's main calls
    /// and, when it is the OM model, its OM calls.
    pub generation: Option<GenerationProfile>,
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

/// The generation behavior one call carries: the same key set appears in
/// `[generation]` (global), `[providers.<p>.models.<m>.generation]`
/// (per-model), and `[om.generation]` (the OM role). Resolution is key by
/// key, role first, then model, then global (#407). `None` = inherit;
/// `extra_body` rides the wire request verbatim (the provider-specific
/// knobs — mastra's `thinkingBudget` is this hole).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GenerationProfile {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub max_tokens: Option<u32>,
    /// The reasoning policy for this role/model; absent = `[thinking]`.
    pub thinking: Option<ThinkingLevel>,
    /// Verbatim passthrough: scalars the endpoint reads and tau does not.
    pub extra_body: BTreeMap<String, ExtraValue>,
}

impl GenerationProfile {
    /// Key-by-key: `self` (the more specific layer) wins, `base` fills
    /// the gaps. `extra_body` merges per key, self winning.
    #[must_use]
    pub fn overlay(&self, base: &Self) -> Self {
        Self {
            temperature: self.temperature.or(base.temperature),
            top_p: self.top_p.or(base.top_p),
            frequency_penalty: self.frequency_penalty.or(base.frequency_penalty),
            presence_penalty: self.presence_penalty.or(base.presence_penalty),
            max_tokens: self.max_tokens.or(base.max_tokens),
            thinking: self.thinking.or(base.thinking),
            extra_body: {
                let mut merged = base.extra_body.clone();
                for (k, v) in &self.extra_body {
                    merged.insert(k.clone(), v.clone());
                }
                merged
            },
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.frequency_penalty.is_none()
            && self.presence_penalty.is_none()
            && self.max_tokens.is_none()
            && self.thinking.is_none()
            && self.extra_body.is_empty()
    }
}

/// One `extra_body` value: a typed scalar (no `serde_json::Value` crosses
/// the config seam, #407).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtraValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
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

/// Per-trigger-point retry counts for the OM provider round-trips
/// (ticket #86): retries after the first failure, before the cycle is
/// abandoned. Mastra's `withRetry` default is 8 (`docs/research/om-async-parity.md` §6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OmRetries {
    pub observe: u32,
    pub buffer: u32,
    pub reflect: u32,
}

impl Default for OmRetries {
    fn default() -> Self {
        Self {
            observe: 8,
            buffer: 8,
            reflect: 8,
        }
    }
}

/// Observational Memory settings; the OM `model` is global, not
/// per-provider (spec §4). #407 renamed `om_model` to `model`; the old key
/// is a load failure naming the successor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Om {
    pub model: String,
    /// #407 retired this spelling: the section is `[om]`, the key is
    /// `model`. Captured solely so the load fails with the migration;
    /// the value is never read.
    #[serde(default)]
    pub om_model: Option<String>,
    /// The OM role's generation profile (#407): what the observer and
    /// reflector calls carry, resolved over the OM model's profile and
    /// the global `[generation]`/`[thinking]`.
    pub generation: GenerationProfile,
    pub observe_threshold: u64,
    pub reflect_threshold: u64,
    pub buffer_increment: u64,
    /// #408 retired this key: the background slice is always on and
    /// `buffer_increment` is the only interval knob. Captured solely so the
    /// load fails naming the successor; the value is never read.
    #[serde(default)]
    pub buffer_tokens: Option<u64>,
    /// Per-trigger-point retry counts for the OM round-trips.
    pub retries: OmRetries,
}

impl Om {
    // The u64 fields are TOML-friendly, but the OM kernel counts tokens in
    // u32: enforce the domain at the parse boundary (ticket #46) so an
    // oversized threshold is a config error, not a launch panic.
    pub(super) fn validate(&self) -> Result<(), LoadError> {
        let limit = u32::MAX;
        for (field, value) in [
            ("om.observe_threshold", self.observe_threshold),
            ("om.reflect_threshold", self.reflect_threshold),
            ("om.buffer_increment", self.buffer_increment),
        ] {
            if value == 0 {
                return Err(LoadError::OmThresholdZero { field });
            }
            if value > u64::from(limit) {
                return Err(LoadError::OmThreshold {
                    field,
                    value,
                    limit,
                });
            }
        }
        Ok(())
    }
}

impl Default for Om {
    fn default() -> Self {
        Self {
            model: String::new(),
            om_model: None,
            generation: GenerationProfile::default(),
            observe_threshold: 30_000,
            reflect_threshold: 40_000,
            buffer_increment: 6_000,
            buffer_tokens: None,
            retries: OmRetries::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubAgents {
    pub max_depth: u32,
    /// Concurrent children; unset means unlimited.
    pub max_concurrent: Option<u32>,
    /// Fleet soft bound: total child handles before spawn-triggered
    /// terminal reclamation evicts the oldest terminal children (#250).
    pub fleet_soft_bound: Option<u32>,
    /// Fleet hard cap on total child handles: a defensive backstop the
    /// soft bound exists to keep under (#250).
    pub fleet_hard_cap: Option<u32>,
}

impl Default for SubAgents {
    fn default() -> Self {
        Self {
            max_depth: 1,
            max_concurrent: None,
            fleet_soft_bound: Some(32),
            fleet_hard_cap: Some(64),
        }
    }
}

impl SubAgents {
    // The soft bound exists to keep the fleet under the hard cap; an
    // inverted pair would make every spawn a cap refusal — a config error,
    // not a runtime surprise (#250).
    pub(super) fn validate(&self) -> Result<(), LoadError> {
        if let (Some(soft), Some(hard)) = (self.fleet_soft_bound, self.fleet_hard_cap)
            && soft > hard
        {
            return Err(LoadError::FleetCapInversion { soft, hard });
        }
        Ok(())
    }
}

/// The completion gate (spec §5.3, design "The mechanical completion gate
/// is the oracle"): a global, mechanical, user-configured check the core
/// runs at every qualifying turn-end. Enforce-always: the section's
/// presence turns the gate on — there is no `off`/`advisory`/`enforce` dial
/// (ADR-0007 Supplement: a deliberate transparency strengthening). The
/// user's only control is *what* the command is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gates {
    /// The check command (a shell line), run with the user's privileges.
    pub command: String,
    /// The check's deadline in seconds.
    pub timeout_secs: u64,
    /// Failed repair turns before the session is marked `gate_failed`.
    pub max_retries: u32,
}

impl Default for Gates {
    fn default() -> Self {
        Self {
            command: String::new(),
            timeout_secs: 300,
            max_retries: 2,
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
    /// Decode-area cap: images above this many pixels are downscaled (#257).
    pub max_pixels: u64,
    /// 1–100, when a read transcodes to JPEG.
    pub jpeg_quality: u8,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_width: 2048,
            max_height: 2048,
            max_bytes: 10 * 1024 * 1024,
            max_pixels: 2_097_152,
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

/// The hashline pin-store GC settings (#225); a `[hashline]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hashline {
    /// Delete pin records not touched within this many days; 0 disables age
    /// expiry (the missing-file sweep always runs).
    pub retention_days: u32,
    /// Run the session-start GC sweeps.
    pub gc_on_start: bool,
}

impl Default for Hashline {
    fn default() -> Self {
        Self {
            retention_days: 180,
            gc_on_start: true,
        }
    }
}
