//! `initialize` and `authenticate`. The `authMethods` list is static on
//! purpose: the registry CI probes the agent in a sandbox with an empty
//! environment, so the advertisement must not depend on configured
//! credentials.

use serde_json::{Value, json};

use crate::config::AcpConfig;
use crate::transport::INVALID_PARAMS;

const METHOD_ID: &str = "openai-compatible";

/// The `initialize` response: protocol v1, the honest capability set (text
/// and `resource_link` prompts only — the baseline every agent must accept;
/// no image/audio/embeddedContext, so rejecting those blocks is
/// spec-compliant), no MCP (ADR-0003), no loadSession.
#[must_use]
pub fn initialize_response() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
            "mcpCapabilities": { "http": false, "sse": false },
        },
        "authMethods": [
            {
                "id": METHOD_ID,
                "name": "OpenAI-compatible endpoint",
                "description": "Point tau at an OpenAI-compatible API (base URL + key)",
            }
        ],
        "agentInfo": {
            "name": "tau",
            "title": "Tau",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// `authenticate`: a no-op success when credentials are already resolvable
/// (config file or the `TAU_*` env); otherwise the `_meta.tau` extension
/// (the codex-acp in-band pattern) installs a connection-scoped provider.
/// The user's config file is never written.
pub async fn authenticate(
    params: Option<&Value>,
    config: &AcpConfig,
) -> Result<Value, (i64, String)> {
    let method_id = params
        .and_then(|p| p.get("methodId"))
        .and_then(Value::as_str)
        .unwrap_or(METHOD_ID);
    if method_id != METHOD_ID {
        return Err((INVALID_PARAMS, format!("unknown auth method {method_id:?}")));
    }
    if config.credentials_resolvable() {
        return Ok(json!({}));
    }
    let Some(provider) = AcpConfig::meta_provider_from(params) else {
        return Err((
            INVALID_PARAMS,
            "no resolvable credentials: set TAU_BASE_URL/TAU_MODEL (and TAU_API_KEY), \
             or pass _meta.tau {baseUrl, model, apiKey?} to authenticate"
                .to_owned(),
        ));
    };
    config.install_meta(provider).await;
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_a_static_agent_auth_method() {
        let resp = initialize_response();
        assert_eq!(resp["protocolVersion"], 1);
        let methods = resp["authMethods"].as_array().expect("authMethods list");
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0]["id"], METHOD_ID);
        // No explicit `type`: the default is `agent`, which the registry
        // CI accepts.
        assert!(methods[0].get("type").is_none());
        assert_eq!(
            resp["agentCapabilities"]["promptCapabilities"]["image"],
            false
        );
        assert_eq!(resp["agentCapabilities"]["mcpCapabilities"]["http"], false);
        assert_eq!(resp["agentInfo"]["name"], "tau");
    }

    #[tokio::test]
    async fn authenticate_unknown_method_is_invalid_params() {
        let config = AcpConfig::test_empty();
        let err = authenticate(Some(&json!({ "methodId": "nope" })), &config)
            .await
            .unwrap_err();
        assert_eq!(err.0, INVALID_PARAMS);
    }
}
