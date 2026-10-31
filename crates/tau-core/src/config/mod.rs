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

/// Observational Memory settings; `om_model` is global, not per-provider (spec §4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Om {
    pub om_model: String,
    pub observe_threshold: u64,
    pub reflect_threshold: u64,
    pub buffer_increment: u64,
    /// Async buffer-interval token count (mastra `bufferTokens`); 0 disables
    /// the async boundary trigger entirely (mastra's own enable condition).
    /// Consumed by the mid-loop boundary trigger that lands with #86 P2.
    pub buffer_tokens: u64,
    /// Per-trigger-point retry counts for the OM round-trips.
    pub retries: OmRetries,
}

impl Om {
    // The u64 fields are TOML-friendly, but the OM kernel counts tokens in
    // u32: enforce the domain at the parse boundary (ticket #46) so an
    // oversized threshold is a config error, not a launch panic.
    fn validate(&self) -> Result<(), LoadError> {
        let limit = u32::MAX;
        for (field, value) in [
            ("om.observe_threshold", self.observe_threshold),
            ("om.reflect_threshold", self.reflect_threshold),
            ("om.buffer_increment", self.buffer_increment),
            ("om.buffer_tokens", self.buffer_tokens),
        ] {
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
            om_model: String::new(),
            observe_threshold: 30_000,
            reflect_threshold: 40_000,
            buffer_increment: 6_000,
            buffer_tokens: 6_000,
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
    fn validate(&self) -> Result<(), LoadError> {
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
    /// The completion gate; `None` (no `[gates]` section) = no gate.
    pub gates: Option<Gates>,
    /// The hashline pin-store GC settings (#225).
    pub hashline: Hashline,
}
impl Config {
    /// Every model declares a window (#338): the first model without one is a
    /// config error, named by provider and model id, so the user's edit lands
    /// on the entry that needs it. The check runs on the merged config, so a
    /// project layer that replaces a provider entry wholesale must carry the
    /// fact the system entry had.
    ///
    /// A declared output cap of `0` is refused in the same pass (N6, #338): the
    /// cap reaches the gate as `Ready { cap: 0 }`, so every call of that session
    /// refuses at the wire with `context_room` — a config that arms a gate
    /// guaranteed to refuse loads as a config error instead.
    fn validate_providers(&self) -> Result<(), LoadError> {
        for (name, provider) in &self.providers {
            for (model, def) in &provider.models {
                if def.context_window == 0 {
                    return Err(LoadError::MissingContextWindow {
                        provider: name.clone(),
                        model: model.clone(),
                    });
                }
                if def.max_tokens == Some(0) {
                    return Err(LoadError::ZeroMaxTokens {
                        key: format!("providers.{name}.models.{model}"),
                    });
                }
            }
        }
        if self.generation.max_tokens == Some(0) {
            return Err(LoadError::ZeroMaxTokens {
                key: "generation".to_owned(),
            });
        }
        Ok(())
    }
}
/// A model id's route, owned: the id a session carries and the provider its id
/// names (§6.7, #336). `build_live` resolves it once at the open; the endpoint,
/// the turn's model facts, and the child factory all follow it — never the
/// provider the config's map sorts first.
#[derive(Debug, Clone)]
pub struct Route {
    /// The config key that owns the provider (§6.7): `Provider` is unnamed — the
    /// map keys it — and §6.7's answer to a contested bare key is a fact about
    /// the provider's identity (#348).
    pub name: String,
    /// The model id, spelled as the session's record names it.
    pub model: String,
    /// The provider that owns the id.
    pub provider: Provider,
}

/// The provider a model id resolves to: its configured name and entry.
#[derive(Debug, Clone, Copy)]
pub struct ProviderRef<'c> {
    /// The provider's name, as the config declares it.
    pub name: &'c str,
    /// The provider's entry.
    pub provider: &'c Provider,
}
/// Why a model id routes nowhere: its prefix names no configured provider, so
/// the id addresses no endpoint (§6.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnownedPrefix(pub String);

impl fmt::Display for UnownedPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "model names no configured provider '{}'", self.0)
    }
}

impl std::error::Error for UnownedPrefix {}

impl From<UnownedPrefix> for tau_protocol::ProtocolError {
    fn from(unowned: UnownedPrefix) -> Self {
        Self::Other {
            message: unowned.to_string(),
        }
    }
}
/// Why `[om].om_model` addresses no endpoint (#348). The memory passes run on
/// the config's own id, not the session's, so a miss is the config author's
/// mistake and the text names the config key: the `om_failed { stage: "route" }`
/// record it becomes is read by whoever edited the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmRouteMiss {
    /// The prefix names no configured provider (§6.7).
    UnownedPrefix { model: String, prefix: String },
    /// The key is declared nowhere: by the provider the prefix names, when the
    /// id carries one, and by no provider at all when it does not.
    Undeclared {
        model: String,
        provider: Option<String>,
    },
    /// Several providers declare the bare key and none of them is the provider
    /// the session is on: the prefix is what disambiguates (§6.7).
    Ambiguous {
        model: String,
        providers: Vec<String>,
    },
}

impl fmt::Display for OmRouteMiss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnownedPrefix { model, prefix } => {
                write!(
                    f,
                    "om.om_model '{model}': provider '{prefix}' is not configured"
                )
            }
            Self::Undeclared {
                model,
                provider: Some(provider),
            } => write!(
                f,
                "om.om_model '{model}': provider '{provider}' declares no model '{}'",
                Config::model_key_for(provider, model)
            ),
            Self::Undeclared { model, .. } => {
                write!(
                    f,
                    "om.om_model '{model}': no configured provider declares it"
                )
            }
            Self::Ambiguous { model, providers } => write!(
                f,
                "om.om_model '{model}': declared by {} — name it as provider/key",
                providers.join(", ")
            ),
        }
    }
}

impl std::error::Error for OmRouteMiss {}

impl Config {
    /// §6.7: the one read of a model id. A model menu names a model as
    /// `provider/key`, because a workspace can reach several providers; the
    /// config declares each model by bare key and the endpoint is addressed by
    /// that key. A prefix naming a configured provider selects it and the
    /// remainder is its key; any other id — a bare key, or a gateway key that
    /// carries `/` itself under a provider nobody declared — is its own key.
    pub(crate) fn model_key<'a>(&self, id: &'a str) -> &'a str {
        match self.provider_for(id) {
            Some(owner) => Self::model_key_for(owner.name, id),
            None => id,
        }
    }

    /// §6.7: the provider entry an id's prefix names, when the prefix is a
    /// configured provider; `None` for a bare key.
    pub(crate) fn provider_for(&self, id: &str) -> Option<ProviderRef<'_>> {
        let (prefix, _) = id.split_once('/')?;
        self.providers
            .get_key_value(prefix)
            .map(|(name, provider)| ProviderRef { name, provider })
    }

    /// §6.7: the provider a model id is addressed to. A prefix naming a
    /// configured provider owns the id. A bare key belongs to the one provider
    /// that declares it; a key several providers declare — and one no provider
    /// declares — stays with `current`, the provider the session already
    /// addresses, because the prefix is what disambiguates between providers and
    /// the core never guesses across them. A prefix naming no configured
    /// provider is a refusal, never a silent route to whichever provider the
    /// config's map sorts first. The key that provider declares is
    /// [`Config::model_key_for`].
    pub(crate) fn route_model<'c>(
        &'c self,
        id: &str,
        current: ProviderRef<'c>,
    ) -> Result<ProviderRef<'c>, UnownedPrefix> {
        if let Some(owner) = self.provider_for(id) {
            return Ok(owner);
        }
        if let Some((prefix, _)) = id.split_once('/') {
            return Err(UnownedPrefix(prefix.to_owned()));
        }
        let mut declaring = self
            .providers
            .iter()
            .filter(|(_, p)| p.models.contains_key(id))
            .map(|(name, provider)| ProviderRef { name, provider });
        match (declaring.next(), declaring.next()) {
            (Some(owner), None) => Ok(owner),
            _ => Ok(current),
        }
    }

    /// §6.7: the provider a child is addressed at (#336). A child's model id
    /// routes like any other; an id that names no provider keeps `current`, the
    /// parent's entry — the spawn already resolved that id against it, so there
    /// is nothing to refuse.
    pub(crate) fn route_child<'c>(&'c self, id: &str, current: ProviderRef<'c>) -> &'c Provider {
        match self.route_model(id, current) {
            Ok(owner) => owner.provider,
            Err(_) => current.provider,
        }
    }
    /// §6.7 for `[om].om_model` (#348): the endpoint the Observer and the
    /// Reflector are addressed at. A prefix naming a configured provider owns
    /// the id, and the key it asks that provider for is the prefix stripped;
    /// a bare key goes to the one provider that declares it. Unlike a session
    /// model, an id that resolves nowhere is a refusal: the memory passes have
    /// no address of their own to fall back to, so `current` — the provider the
    /// session is on — is only an answer for a key that provider declares among
    /// several that do.
    /// It walks §6.7's rules itself rather than reading through `route_model`
    /// because that answers the no-declarer and contested cases with `current`,
    /// and `om_model` refuses both.
    pub(crate) fn route_om<'c>(
        &'c self,
        om_model: &str,
        current: ProviderRef<'c>,
    ) -> Result<ProviderRef<'c>, OmRouteMiss> {
        if let Some(owner) = self.provider_for(om_model) {
            let key = Self::model_key_for(owner.name, om_model);
            return if owner.provider.models.contains_key(key) {
                Ok(owner)
            } else {
                Err(OmRouteMiss::Undeclared {
                    model: om_model.to_owned(),
                    provider: Some(owner.name.to_owned()),
                })
            };
        }
        if let Some((prefix, _)) = om_model.split_once('/') {
            return Err(OmRouteMiss::UnownedPrefix {
                model: om_model.to_owned(),
                prefix: prefix.to_owned(),
            });
        }
        let declaring: Vec<ProviderRef<'_>> = self
            .providers
            .iter()
            .filter(|(_, provider)| provider.models.contains_key(om_model))
            .map(|(name, provider)| ProviderRef { name, provider })
            .collect();
        let Some(only) = declaring.first() else {
            return Err(OmRouteMiss::Undeclared {
                model: om_model.to_owned(),
                provider: None,
            });
        };
        if declaring.len() == 1 {
            return Ok(*only);
        }
        if current.provider.models.contains_key(om_model) {
            return Ok(current);
        }
        // The refusal is the config author's fix instruction, so it names every
        // provider in the collision, in a stable order, not the one the map
        // happened to reach first.
        let mut providers = declaring
            .iter()
            .map(|owner| owner.name.to_owned())
            .collect::<Vec<_>>();
        providers.sort();
        Err(OmRouteMiss::Ambiguous {
            model: om_model.to_owned(),
            providers,
        })
    }

    /// §6.7: the key the provider named `name` declares for `id` — the id with
    /// that provider's prefix stripped, and the id itself for every other
    /// spelling (a bare key, or a gateway key carrying `/` under a foreign
    /// prefix).
    pub(crate) fn model_key_for<'a>(name: &str, id: &'a str) -> &'a str {
        id.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(id)
    }
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
    gates: Option<Gates>,
    hashline: Option<Hashline>,
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
        if let Some(gates) = &self.gates {
            // A bare `[gates]` (blank `command`) is a no-gate, not a silent
            // no-op gate: `bash -c ""` exits 0, which would pass every
            // qualifying turn while the prompt claims verification.
            config.gates = if gates.command.trim().is_empty() {
                None
            } else {
                Some(gates.clone())
            };
        }
        if let Some(hashline) = &self.hashline {
            config.hashline = hashline.clone();
        }
    }
}

/// The config layer's file name inside its layer directory (spec §12). The
/// watcher filters its batch on this name: the layer dirs carry the sessions,
/// agents, and plans too, and only this file is config.
pub const CONFIG_FILE: &str = "config.toml";

/// The project layer's stand-in for "none": a path that cannot exist, so `load`
/// answers with the system layer alone. Boot, the ACP entry, subagent-type
/// discovery, and the system-layer reload all read config with no project open.
pub const NO_PROJECT_LAYER: &str = "/nonexistent-tau-project";

/// Load the merged config from the system directory and the project directory.
/// Sections layer over the system values; a project provider entry of the same
/// name replaces the system entry wholesale.
pub fn load(system_dir: &Path, project_dir: &Path) -> Result<Config, LoadError> {
    let system = ConfigFile::read(&system_dir.join(CONFIG_FILE))?;
    let project = ConfigFile::read(&project_dir.join(CONFIG_FILE))?;
    let mut config = Config::default();
    system.merge_into(&mut config);
    project.merge_into(&mut config);
    config.om.validate()?;
    config.subagents.validate()?;
    config.validate_providers()?;
    Ok(config)
}

impl LoadError {
    /// The layer file a parse failure names. A validation failure (an `om`
    /// threshold, a fleet-cap inversion, a model with no window) is the merged
    /// config's property, not one file's, so it names no layer (#337).
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Parse(path, _) => Some(path),
            Self::Io(_)
            | Self::OmThreshold { .. }
            | Self::MissingContextWindow { .. }
            | Self::ZeroMaxTokens { .. }
            | Self::FleetCapInversion { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Parse(std::path::PathBuf, toml::de::Error),
    /// An `[om]` threshold above `u32::MAX` (ticket #46).
    OmThreshold {
        field: &'static str,
        value: u64,
        limit: u32,
    },
    /// A model with no declared `context_window` (#338): the context gate is
    /// computed from that fact, so a config that omits it leaves the session's
    /// prompts unbounded. A load-time refusal, never a silent no-gate.
    MissingContextWindow {
        provider: String,
        model: String,
    },
    /// `subagents.fleet_soft_bound` above `fleet_hard_cap` (#250).
    FleetCapInversion {
        soft: u32,
        hard: u32,
    },
    /// An output cap declared as `0` (#338, N6): `key` is the config's own
    /// spelling of the entry (`providers.<name>.models.<model>`, `generation`).
    /// A cap of 0 arms a gate every call refuses with `context_room`, so the
    /// config is the error, not the session.
    ZeroMaxTokens {
        key: String,
    },
}
impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading config: {e}"),
            Self::Parse(path, e) => write!(f, "parsing {}: {e}", path.display()),
            Self::OmThreshold {
                field,
                value,
                limit,
            } => {
                write!(f, "{field} = {value} exceeds the u32 token limit ({limit})")
            }
            Self::MissingContextWindow { provider, model } => write!(
                f,
                "providers.{provider}.models.{model} declares no context_window; the context gate needs it"
            ),
            Self::ZeroMaxTokens { key } => {
                write!(
                    f,
                    "{key} declares max_tokens = 0; the output cap must be positive"
                )
            }
            Self::FleetCapInversion { soft, hard } => {
                write!(
                    f,
                    "subagents.fleet_soft_bound ({soft}) must not exceed fleet_hard_cap ({hard})"
                )
            }
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
