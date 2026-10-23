//! The ACP server: the stdin dispatch loop and the session methods. Every
//! stdout byte is an ACP message; diagnostics go to stderr.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use serde_json::Value;
use tau_core::harness::{Core, CoreError};
use tau_protocol::{Command, CommandOutput};
use tokio::io::{AsyncBufReadExt, AsyncRead};

use self::session_methods::{session_cancel, session_new, session_prompt, set_config_option};
use crate::auth;
use crate::config::AcpConfig;
use crate::pump;
use crate::sessions::Registry;
use crate::transport::{
    self, INTERNAL_ERROR, INVALID_REQUEST, METHOD_NOT_FOUND, Out, PARSE_ERROR, Rpc,
};

/// Everything the server tasks share (the `Arc`s make the `Clone` cheap;
/// the spawned settle waiter and the pump each keep one).
#[derive(Clone)]
pub struct Server {
    pub config: std::sync::Arc<crate::config::AcpConfig>,
    pub out: Out,
    pub registry: std::sync::Arc<Registry>,
    /// The core is built lazily at first use: an `authenticate` `_meta`
    /// provider must land before the first session's model resolution.
    core: std::sync::Arc<tokio::sync::Mutex<Option<Arc<Core>>>>,
}

impl Server {
    /// Build the core on first use and start the event pump with it. The
    /// lock is held across the build: a concurrent first use must queue,
    /// not build a second core (a second `pump::run` would panic in
    /// `Core::events()`, which hands out its receiver exactly once).
    pub async fn ensure_core(&self) -> Result<Arc<Core>, CoreError> {
        let mut slot = self.core.lock().await;
        if let Some(core) = &*slot {
            return Ok(core.clone());
        }
        let core = self.config.build_core().await?;
        let pump_out = self.out.clone();
        let pump_registry = self.registry.clone();
        tokio::spawn(pump::run(core.clone(), pump_out, pump_registry));
        *slot = Some(core.clone());
        Ok(core)
    }

    /// Dispatch `cmd`, answering a JSON-RPC internal-error response on a
    /// `ProtocolError`; `None` once that error has been sent (the caller
    /// returns).
    async fn dispatch(&self, id: &Value, cmd: Command) -> Option<CommandOutput> {
        let core = match self.ensure_core().await {
            Ok(core) => core,
            Err(e) => {
                self.out.send(&transport::error(
                    id,
                    INTERNAL_ERROR,
                    &format!("core build failed: {e}"),
                ));
                return None;
            }
        };
        match core.dispatch(cmd) {
            Ok(out) => Some(out),
            Err(e) => {
                self.out
                    .send(&transport::error(id, INTERNAL_ERROR, &e.to_string()));
                None
            }
        }
    }
}

/// Run the server over the given streams until stdin closes.
pub async fn serve(
    stdin: impl AsyncRead + Unpin,
    stdout: impl Write + Send + 'static,
) -> Result<(), CoreError> {
    let server = Server {
        config: std::sync::Arc::new(AcpConfig::load()?),
        out: Out::new(stdout),
        registry: std::sync::Arc::new(Registry::new(HashMap::new())),
        core: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
    };
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        dispatch_line(&server, &line).await;
    }
    Ok(())
}

async fn dispatch_line(server: &Server, line: &str) {
    let Some(rpc) = serde_json::from_str::<Rpc>(line).ok() else {
        // A malformed frame has no id to echo: a null-id parse error.
        server
            .out
            .send(&transport::error(&Value::Null, PARSE_ERROR, "parse error"));
        return;
    };
    let Some(method) = rpc.method.clone() else {
        if let Some(id) = &rpc.id {
            server
                .out
                .send(&transport::error(id, INVALID_REQUEST, "missing method"));
        }
        return;
    };
    match method.as_str() {
        "initialize" => {
            if let Some(id) = &rpc.id {
                server
                    .out
                    .send(&transport::result(id, &auth::initialize_response()));
            }
        }
        "authenticate" => {
            let Some(id) = &rpc.id else {
                return;
            };
            match auth::authenticate(rpc.params.as_ref(), &server.config).await {
                Ok(result) => server.out.send(&transport::result(id, &result)),
                Err((code, message)) => {
                    server.out.send(&transport::error(id, code, &message));
                }
            }
        }
        "session/new" => session_new(server, &rpc).await,
        "session/prompt" => session_prompt(server, &rpc).await,
        "session/cancel" => session_cancel(server, &rpc).await,
        "session/set_config_option" => set_config_option(server, &rpc).await,
        other => {
            if let Some(id) = &rpc.id {
                server.out.send(&transport::error(
                    id,
                    METHOD_NOT_FOUND,
                    &format!("unknown method {other:?}"),
                ));
            }
        }
    }
}

mod session_methods;
