//! OpenAI-compatible provider surface (spec §6): v0 talks to one API family —
//! the `responses` endpoint plus the plain REST endpoints like `/models` that
//! every OpenAI-compatible server implements. No provider detection, no
//! catalog, no keychain.

use crate::config::Provider;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

/// A model as reported by the provider's `/models` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
}

#[derive(Debug)]
pub enum ProviderError {
    Request(reqwest::Error),
    Status { status: u16, body: String },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(e) => write!(f, "provider request failed: {e}"),
            Self::Status { status, body } => write!(f, "provider returned {status}: {body}"),
        }
    }
}

impl Error for ProviderError {}

/// Join the API root with a sub-path, tolerating a trailing slash on `base`.
fn endpoint_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

/// `GET {base_url}/models` — the round-trip that proves provider wiring.
pub async fn list_models(
    client: &reqwest::Client,
    provider: &Provider,
) -> Result<Vec<Model>, ProviderError> {
    let mut request = client.get(endpoint_url(&provider.base_url, "models"));
    let key = if provider.key_env.is_empty() {
        None
    } else {
        std::env::var(&provider.key_env)
            .ok()
            .filter(|k| !k.is_empty())
    };
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(ProviderError::Request)?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status, body });
    }
    let payload: ModelsResponse = response.json().await.map_err(ProviderError::Request)?;
    Ok(payload.data)
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<Model>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_trims_trailing_slash() {
        assert_eq!(endpoint_url("http://x/v1/", "models"), "http://x/v1/models");
        assert_eq!(endpoint_url("http://x/v1", "models"), "http://x/v1/models");
    }

    /// Live round-trip against a local OpenAI-compatible server; skipped unless
    /// TAU_TEST_ENDPOINT is set (CI has no model server).
    #[tokio::test]
    async fn models_roundtrip() {
        let base = match std::env::var("TAU_TEST_ENDPOINT") {
            Ok(base) => base,
            Err(_) => return,
        };
        let client = reqwest::Client::new();
        let provider = Provider {
            base_url: base,
            key_env: String::new(),
            models: Vec::new(),
        };
        let models = list_models(&client, &provider)
            .await
            .expect("models round-trip");
        assert!(!models.is_empty());
    }
}
