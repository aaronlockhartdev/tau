//! ACP-mode configuration: the user's real config (the same load the app
//! and eval rig use), the `TAU_*` env-override provider (the Harbor
//! channel), and the connection-scoped `authenticate` `_meta` provider.

use std::collections::BTreeMap;
use std::path::Path;

use tau_core::config::{Config, ModelDef, Provider};
use tau_core::harness::{Core, CoreBuilder};
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
    pub fn load() -> Self {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("HOME set");
        let system_dir = home.join(".config").join("tau");
        // No project at startup: the core layers the workspace's
        // `.tau/config.toml` itself at workspace open (spec §12).
        let base = tau_core::config::load(&system_dir, Path::new("/nonexistent-tau-project"))
            .unwrap_or_default();
        Self {
            base,
            system_dir,
            meta: TokioMutex::new(None),
        }
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
        models.insert(model, ModelDef::default());
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
    pub async fn build_core(&self) -> std::sync::Arc<Core> {
        let mut builder = CoreBuilder::default_system();
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

    /// The model selector's options for one workspace: every configured model
    /// id (file layers + env + `_meta`), and the effective default
    /// (`generation.default_model` winning, else the first provider's first
    /// model — the launch.rs rule).
    pub async fn model_options_for(&self, cwd: &str) -> (Vec<String>, Option<String>) {
        let file = self.file_layers(cwd);
        let mut merged = file.providers;
        if let Some(p) = self.env_provider() {
            merged.insert(ENV_PROVIDER.to_owned(), p);
        }
        if let Some(p) = self.meta.lock().await.clone() {
            merged.insert(META_PROVIDER.to_owned(), p);
        }
        let models = merged
            .values()
            .flat_map(|p| p.models.keys().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let default = merged.values().next().and_then(|first| {
            let wanted = &file.generation.default_model;
            if !wanted.is_empty() && first.models.contains_key(wanted) {
                Some(wanted.clone())
            } else {
                first.models.keys().next().cloned()
            }
        });
        (models, default)
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
        models.insert(model, ModelDef::default());
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
}
