//! ACP-mode configuration: the user's real config (the same load the app
//! and eval rig use), the `TAU_*` env-override provider (the Harbor
//! channel), and the connection-scoped `authenticate` `_meta` provider.

use std::collections::BTreeMap;
use std::path::Path;

use tau_core::config::{Config, Provider};
use tau_core::harness::{Core, CoreBuilder, CoreError};
use tokio::sync::Mutex as TokioMutex;

/// The env contract (frozen once the registry entry ships): a base URL and
/// a model id layer a synthetic provider; the key, when present, is read
/// from `TAU_API_KEY` at request time.
const ENV_PROVIDER: &str = "acp-env";
const META_PROVIDER: &str = "acp-meta";

/// What `tau acp` knows about models before any session exists. The core is
/// built lazily (first `session/new`), so a connection-scoped `_meta`
/// provider installed by `authenticate` is folded in before the first
/// session's model resolution sees it.
pub struct AcpConfig {
    base: Config,
    /// The system config dir, kept so a workspace's project layer can be
    /// loaded the same way the core does at workspace open (M4).
    system_dir: std::path::PathBuf,
    /// `authenticate` `_meta` provider, set after load; the core is not
    /// built yet when it arrives (authenticate precedes session/new).
    meta: TokioMutex<Option<Provider>>,
}

impl AcpConfig {
    /// Load the user's real config and capture the env overrides.
    pub fn load() -> Result<Self, CoreError> {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return Err(CoreError(
                "home is not set; cannot locate the system config dir".into(),
            ));
        };
        let system_dir = home.join(".config").join("tau");
        // No project at startup: the core layers the workspace's
        // `.tau/config.toml` itself at workspace open (spec §12).
        let base =
            tau_core::config::load(&system_dir, Path::new(tau_core::config::NO_PROJECT_LAYER))
                .unwrap_or_default();
        Ok(Self {
            base,
            system_dir,
            meta: TokioMutex::new(None),
        })
    }

    /// The `TAU_*` env-override provider, when the contract is met
    /// (base URL + model id; the key is optional — local endpoints need
    /// none).
    pub fn env_provider(&self) -> Option<Provider> {
        let base_url = std::env::var("TAU_BASE_URL").ok()?;
        let model = std::env::var("TAU_MODEL").ok()?;
        let key_env = if std::env::var("TAU_API_KEY").is_ok() {
            "TAU_API_KEY".to_owned()
        } else {
            String::new()
        };
        let mut models = BTreeMap::new();
        // The channel names no facts, and #338 requires a window: the entry
        // declares the default pair rather than riding in ungated.
        models.insert(model, Provider::default_def());
        Some(Provider {
            base_url,
            key_env,
            models,
        })
    }

    /// The file layers as seen from a workspace: the system config plus the
    /// project's `.tau/config.toml` — the same load the core performs at
    /// workspace open (spec §12), so the model selector sees project-layer
    /// providers and a project-level default model.
    fn file_layers(&self, cwd: &str) -> Config {
        let project = Path::new(cwd).join(".tau");
        if project.exists() {
            tau_core::config::load(&self.system_dir, &project).unwrap_or_else(|_| self.base.clone())
        } else {
            self.base.clone()
        }
    }

    /// Build the production-shape core with the layered providers.
    pub async fn build_core(&self) -> Result<std::sync::Arc<Core>, CoreError> {
        let mut builder = CoreBuilder::default_system()?;
        if let Some(p) = self.env_provider() {
            builder = builder.with_provider(ENV_PROVIDER.to_owned(), p);
        }
        if let Some(p) = self.meta.lock().await.clone() {
            builder = builder.with_provider(META_PROVIDER.to_owned(), p);
        }
        builder.build()
    }

    /// Whether a key is resolvable right now (a keyless provider needs none;
    /// otherwise the provider's `key_env` must be set in the environment,
    /// or the env override carries one). Drives the `authenticate` no-op.
    pub fn credentials_resolvable(&self) -> bool {
        self.env_provider()
            .is_some_and(|p| p.key_env.is_empty() || std::env::var(&p.key_env).is_ok())
            || self
                .base
                .providers
                .values()
                .any(|p| p.key_env.is_empty() || std::env::var(&p.key_env).is_ok())
    }

    /// The file layers as seen from a workspace, with the env and `_meta`
    /// providers layered on — the merge the model selector and the window
    /// lookup share.
    async fn merged(&self, cwd: &str) -> Config {
        let mut merged = self.file_layers(cwd);
        if let Some(p) = self.env_provider() {
            merged.providers.insert(ENV_PROVIDER.to_owned(), p);
        }
        if let Some(p) = self.meta.lock().await.clone() {
            merged.providers.insert(META_PROVIDER.to_owned(), p);
        }
        merged
    }

    /// The model selector's options for one workspace: every configured model
    /// id (file layers + env + `_meta`), and the effective default
    /// (`generation.default_model` winning, else the first provider's first
    /// model — the launch.rs rule).
    pub async fn model_options_for(&self, cwd: &str) -> (Vec<String>, Option<String>) {
        let merged = self.merged(cwd).await;
        let models: Vec<String> = merged
            .providers
            .values()
            .flat_map(|p| p.models.keys().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        // The default comes from the first provider, but that provider may
        // be configured with an empty models table while a later one has
        // models: fall back to the first listed model so a non-empty list
        // always carries a default (session/new must not panic on config).
        let default = merged
            .providers
            .values()
            .next()
            .and_then(|first| {
                let wanted = &merged.generation.default_model;
                if !wanted.is_empty() && first.models.contains_key(wanted) {
                    Some(wanted.clone())
                } else {
                    first.models.keys().next().cloned()
                }
            })
            .or_else(|| models.first().cloned());
        (models, default)
    }

    /// The model's declared context window, when any layer declares one: a
    /// layer that offers the model without a window is skipped, not treated
    /// as "no window" (N5 — the `usage_update` `size` must not be guessed).
    /// Config requires the fact (#338), so a 0 is an entry that reached the
    /// map without the load's validation, not a 0-token model.
    #[must_use]
    pub async fn context_window_for(&self, cwd: &str, model: &str) -> Option<u64> {
        self.merged(cwd)
            .await
            .providers
            .values()
            .filter_map(|p| p.models.get(model))
            .map(|m| u64::from(m.context_window))
            .find(|window| *window > 0)
    }
    /// `authenticate` `_meta` install: connection-scoped, never written to
    /// the user's config file.
    pub async fn install_meta(&self, provider: Provider) {
        *self.meta.lock().await = Some(provider);
    }
    /// A `_meta` provider built from the `authenticate` params'
    /// `_meta.tau` extension (`{baseUrl, model}`; the codex-acp in-band
    /// pattern). The key is not carried in-band: the core resolves provider
    /// keys from the environment, so an in-band key would have no channel.
    /// `None` when the extension is absent or incomplete.
    #[must_use]
    pub fn meta_provider_from(params: Option<&serde_json::Value>) -> Option<Provider> {
        let meta = params?.get("_meta")?.get("tau")?;
        let base_url = meta.get("baseUrl")?.as_str()?.to_owned();
        let model = meta.get("model")?.as_str()?.to_owned();
        let mut models = BTreeMap::new();
        models.insert(model, Provider::default_def());
        Some(Provider {
            base_url,
            key_env: String::new(),
            models,
        })
    }

    /// A provider-less config for the unit tests (no file layer; the env
    /// overrides can still apply, which the tests that need them set).
    #[cfg(test)]
    #[must_use]
    pub fn test_empty() -> Self {
        Self {
            base: Config::default(),
            system_dir: std::path::PathBuf::from("/nonexistent-tau-system"),
            meta: TokioMutex::new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_core::config::ModelDef;
    #[test]
    fn meta_provider_parses_the_extension() {
        let params = serde_json::json!({
            "methodId": "openai-compatible",
            "_meta": { "tau": { "baseUrl": "http://x/v1", "model": "m1" } }
        });
        let p = AcpConfig::meta_provider_from(Some(&params)).expect("parses");
        assert_eq!(p.base_url, "http://x/v1");
        assert!(p.key_env.is_empty());
        assert!(p.models.contains_key("m1"));
    }

    #[test]
    fn meta_provider_rejects_incomplete_extension() {
        // No _meta at all, no baseUrl, no model: each is an absence, not a
        // half-installed provider.
        assert!(
            AcpConfig::meta_provider_from(Some(&serde_json::json!({ "methodId": "x" }))).is_none()
        );
        assert!(
            AcpConfig::meta_provider_from(Some(&serde_json::json!({
                "_meta": { "tau": { "model": "m1" } }
            })))
            .is_none()
        );
        assert!(
            AcpConfig::meta_provider_from(Some(&serde_json::json!({
                "_meta": { "tau": { "baseUrl": "http://x/v1" } }
            })))
            .is_none()
        );
    }

    #[tokio::test]
    async fn context_window_resolves_across_layers() {
        let mut c = AcpConfig::test_empty();
        // A layer that offers the model without a window...
        let mut bare = Provider {
            base_url: "http://a/v1".into(),
            key_env: String::new(),
            models: BTreeMap::new(),
        };
        bare.models.insert("m1".into(), ModelDef::default());
        c.base.providers.insert("bare".into(), bare);
        // ...and one that declares it: the declaration wins over the bare
        // offering; an unknown model has no window.
        let mut full = Provider {
            base_url: "http://b/v1".into(),
            key_env: String::new(),
            models: BTreeMap::new(),
        };
        let def = ModelDef {
            context_window: 4096,
            ..Default::default()
        };
        full.models.insert("m1".into(), def);
        c.base.providers.insert("full".into(), full);
        assert_eq!(c.context_window_for("/p", "m1").await, Some(4096));
        assert_eq!(c.context_window_for("/p", "nope").await, None);
    }

    #[tokio::test]
    async fn a_non_empty_model_list_always_carries_a_default() {
        // Regression: a config whose alphabetically-first provider has an
        // empty models table used to leave `default` None for a non-empty
        // list — session/new then panicked on the expect.
        let mut c = AcpConfig::test_empty();
        let empty = Provider {
            base_url: "http://a/v1".into(),
            key_env: String::new(),
            models: BTreeMap::new(),
        };
        c.base.providers.insert("aaa-empty".into(), empty);
        let mut full = Provider {
            base_url: "http://b/v1".into(),
            key_env: String::new(),
            models: BTreeMap::new(),
        };
        full.models.insert("m1".into(), ModelDef::default());
        c.base.providers.insert("bbb-full".into(), full);

        let (models, default) = c.model_options_for("/p").await;
        assert_eq!(models, vec!["m1".to_owned()]);
        assert_eq!(default.as_deref(), Some("m1"));

        // Negative: no models anywhere — no list, no default.
        let (models, default) = AcpConfig::test_empty().model_options_for("/p").await;
        assert!(models.is_empty());
        assert!(default.is_none());
    }
}
