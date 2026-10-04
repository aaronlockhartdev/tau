//! The ACP server: the stdin dispatch loop and the session methods. Every
//! stdout byte is an ACP message; diagnostics go to stderr.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tau_core::harness::Core;
use tau_protocol::{Command, CommandOutput, MessageLane, ProtocolError};
use tokio::io::{AsyncBufReadExt, AsyncRead};
use tokio::sync::oneshot;

use crate::auth;
use crate::config::AcpConfig;
use crate::mapper::Mapper;
use crate::pump;
use crate::sessions::{Outcome, Registry, SessionState};
use crate::transport::{
    self, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, Out, PARSE_ERROR, Rpc,
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
    core: std::sync::Arc<Mutex<Option<Arc<Core>>>>,
}

impl Server {
    /// Build the core on first use and start the event pump with it.
    pub async fn ensure_core(&self) -> Arc<Core> {
        {
            let slot = self
                .core
                .lock()
                .expect("core slot: no panic while the lock is held");
            if let Some(core) = &*slot {
                return core.clone();
            }
        }
        let core = self.config.build_core().await;
        let pump_out = self.out.clone();
        let pump_registry = self.registry.clone();
        tokio::spawn(pump::run(core.clone(), pump_out, pump_registry));
        *self
            .core
            .lock()
            .expect("core slot: no panic while the lock is held") = Some(core.clone());
        core
    }
}

/// Run the server over the given streams until stdin closes.
pub async fn serve(stdin: impl AsyncRead + Unpin, stdout: impl Write + Send + 'static) {
    let server = Server {
        config: std::sync::Arc::new(AcpConfig::load()),
        out: Out::new(stdout),
        registry: std::sync::Arc::new(Registry::new(HashMap::new())),
        core: std::sync::Arc::new(Mutex::new(None)),
    };
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        dispatch_line(&server, &line).await;
    }
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

fn protocol_message(e: &ProtocolError) -> String {
    match e {
        ProtocolError::Unsupported { message } | ProtocolError::Other { message } => {
            message.clone()
        }
        ProtocolError::NotFound { what } => format!("not found: {what}"),
    }
}

async fn session_new(server: &Server, rpc: &Rpc) {
    let Some(id) = &rpc.id else {
        return;
    };
    let cwd = rpc
        .params
        .as_ref()
        .and_then(|p| p.get("cwd"))
        .and_then(Value::as_str);
    let Some(cwd) = cwd else {
        server.out.send(&transport::error(
            id,
            INVALID_PARAMS,
            "cwd is required (an absolute path)",
        ));
        return;
    };
    let core = server.ensure_core().await;
    let workspace = match core.dispatch(Command::WorkspaceOpen {
        cwd: cwd.to_owned(),
    }) {
        Ok(CommandOutput::Workspace { workspace }) => workspace,
        Ok(_) => unreachable!("dispatch routes the arm"),
        Err(e) => {
            server
                .out
                .send(&transport::error(id, INTERNAL_ERROR, &protocol_message(&e)));
            return;
        }
    };
    let session = match core.dispatch(Command::SessionNew {
        workspace: workspace.id.clone(),
        title: None,
    }) {
        Ok(CommandOutput::Session { session }) => session,
        Ok(_) => unreachable!("dispatch routes the arm"),
        Err(e) => {
            server
                .out
                .send(&transport::error(id, INTERNAL_ERROR, &protocol_message(&e)));
            return;
        }
    };
    // The model selector (Harbor's `--model` path): every configured model
    // id, the effective default current. Absent when no model is configured
    // (an empty select would be a contract violation).
    let (models, default) = server.config.model_options().await;
    let mut result = json!({ "sessionId": session.id });
    if !models.is_empty() {
        let options = models
            .iter()
            .map(|m| json!({ "value": m, "name": m }))
            .collect::<Vec<_>>();
        result["configOptions"] = json!([
            {
                "id": "model",
                "name": "Model",
                "category": "model",
                "type": "select",
                "currentValue": default,
                "options": options,
            }
        ]);
    }
    server
        .registry
        .lock()
        .expect("session registry: no panic while the lock is held")
        .insert(
            session.id.clone(),
            SessionState {
                workspace: workspace.id,
                mapper: Mapper::default(),
                in_flight: false,
                pending: None,
                saw_stream_end: false,
                saw_interrupted: false,
            },
        );
    server.out.send(&transport::result(id, &result));
}

/// The prompt text joined across text blocks. v0 accepts text blocks only:
/// image/audio/embeddedContext are not advertised, so anything else is a
/// client contract violation.
fn prompt_text(params: Option<&Value>) -> Result<String, String> {
    let Some(blocks) = params
        .and_then(|p| p.get("prompt"))
        .and_then(Value::as_array)
    else {
        return Err("prompt is required".to_owned());
    };
    let mut text = String::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
            Some(other) => {
                return Err(format!(
                    "unsupported prompt block type {other:?} (v0 accepts text only)"
                ));
            }
            None => return Err("prompt block missing type".to_owned()),
        }
    }
    if text.is_empty() {
        return Err("empty prompt".to_owned());
    }
    Ok(text)
}

async fn session_prompt(server: &Server, rpc: &Rpc) {
    let Some(id) = &rpc.id else {
        return;
    };
    let params = rpc.params.as_ref();
    let session_id = params
        .and_then(|p| p.get("sessionId"))
        .and_then(Value::as_str);
    let Some(session_id) = session_id else {
        server.out.send(&transport::error(
            id,
            INVALID_PARAMS,
            "sessionId is required",
        ));
        return;
    };
    let text = match prompt_text(params) {
        Ok(text) => text,
        Err(message) => {
            server
                .out
                .send(&transport::error(id, INVALID_PARAMS, &message));
            return;
        }
    };

    let core = server.ensure_core().await;

    // The turn lock and settle registration land before the send: a turn
    // that finished before the registration would otherwise orphan the
    // response (the settle signal would find no pending sender).
    let (tx, rx) = oneshot::channel();
    {
        let mut reg = server
            .registry
            .lock()
            .expect("session registry: no panic while the lock is held");
        let Some(state) = reg.get_mut(session_id) else {
            drop(reg);
            server.out.send(&transport::error(
                id,
                INVALID_PARAMS,
                &format!("unknown session {session_id:?}"),
            ));
            return;
        };
        if state.in_flight {
            drop(reg);
            server.out.send(&transport::error(
                id,
                INVALID_REQUEST,
                "a turn is already in flight for this session",
            ));
            return;
        }
        state.in_flight = true;
        state.saw_stream_end = false;
        state.saw_interrupted = false;
        state.pending = Some(tx);
    }

    if let Err(e) = core.dispatch(Command::MessageSend {
        session: session_id.to_owned(),
        text,
        lane: MessageLane::FollowUp,
    }) {
        // Refused before the turn started: release the lock and answer
        // now, not at a settle that will never come.
        let mut reg = server
            .registry
            .lock()
            .expect("session registry: no panic while the lock is held");
        if let Some(state) = reg.get_mut(session_id) {
            state.in_flight = false;
            state.pending = None;
        }
        server
            .out
            .send(&transport::error(id, INTERNAL_ERROR, &protocol_message(&e)));
        return;
    }

    // The settle waiter: the pump resolves `rx` at the settle signal; this
    // task writes the `session/prompt` response and releases the turn lock.
    let waiter = server.clone();
    let id = id.clone();
    let session_id = session_id.to_owned();
    tokio::spawn(async move {
        let outcome = rx
            .await
            .unwrap_or(Outcome::Error("turn ended without a settle signal".into()));
        let message = match outcome {
            Outcome::EndTurn => transport::result(&id, &json!({ "stopReason": "end_turn" })),
            Outcome::Cancelled => transport::result(&id, &json!({ "stopReason": "cancelled" })),
            Outcome::Error(message) => transport::error(&id, INTERNAL_ERROR, &message),
        };
        waiter.out.send(&message);
        let mut reg = waiter
            .registry
            .lock()
            .expect("session registry: no panic while the lock is held");
        if let Some(state) = reg.get_mut(&session_id) {
            state.in_flight = false;
        }
    });
}

async fn session_cancel(server: &Server, rpc: &Rpc) {
    // A notification: no response, even for a bad session.
    let Some(session_id) = rpc
        .params
        .as_ref()
        .and_then(|p| p.get("sessionId"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let core = server.ensure_core().await;
    if let Err(e) = core.dispatch(Command::MessageStop {
        session: session_id.to_owned(),
    }) {
        eprintln!("acp: cancel for {session_id}: {}", protocol_message(&e));
    }
    // The in-flight prompt (if any) answers `stopReason: "cancelled"` when
    // the interrupted turn settles.
}

async fn set_config_option(server: &Server, rpc: &Rpc) {
    let Some(id) = &rpc.id else {
        return;
    };
    let params = rpc.params.as_ref();
    let session_id = params
        .and_then(|p| p.get("sessionId"))
        .and_then(Value::as_str);
    let config_id = params
        .and_then(|p| p.get("configId"))
        .and_then(Value::as_str);
    let value = params.and_then(|p| p.get("value")).and_then(Value::as_str);
    if config_id != Some("model") || value.is_none() {
        server.out.send(&transport::error(
            id,
            INVALID_PARAMS,
            "v0 supports the 'model' select only",
        ));
        return;
    }
    let Some(session_id) = session_id else {
        server.out.send(&transport::error(
            id,
            INVALID_PARAMS,
            "sessionId is required",
        ));
        return;
    };
    {
        let reg = server
            .registry
            .lock()
            .expect("session registry: no panic while the lock is held");
        if !reg.contains_key(session_id) {
            drop(reg);
            server.out.send(&transport::error(
                id,
                INVALID_PARAMS,
                &format!("unknown session {session_id:?}"),
            ));
            return;
        }
    }
    let core = server.ensure_core().await;
    let value = value.expect("checked above");
    match core.dispatch(Command::SessionSetModel {
        session: session_id.to_owned(),
        model: value.to_owned(),
    }) {
        Ok(_) => server.out.send(&transport::result(id, &json!({}))),
        Err(e) => server
            .out
            .send(&transport::error(id, INTERNAL_ERROR, &protocol_message(&e))),
    }
}
