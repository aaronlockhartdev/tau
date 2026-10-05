//! The ACP server: the stdin dispatch loop and the session methods. Every
//! stdout byte is an ACP message; diagnostics go to stderr.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

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
    core: std::sync::Arc<tokio::sync::Mutex<Option<Arc<Core>>>>,
}

impl Server {
    /// Build the core on first use and start the event pump with it. The
    /// lock is held across the build: a concurrent first use must queue,
    /// not build a second core (a second `pump::run` would panic in
    /// `Core::events()`, which hands out its receiver exactly once).
    pub async fn ensure_core(&self) -> Arc<Core> {
        let mut slot = self.core.lock().await;
        if let Some(core) = &*slot {
            return core.clone();
        }
        let core = self.config.build_core().await;
        let pump_out = self.out.clone();
        let pump_registry = self.registry.clone();
        tokio::spawn(pump::run(core.clone(), pump_out, pump_registry));
        *slot = Some(core.clone());
        core
    }
}

/// Run the server over the given streams until stdin closes.
pub async fn serve(stdin: impl AsyncRead + Unpin, stdout: impl Write + Send + 'static) {
    let server = Server {
        config: std::sync::Arc::new(AcpConfig::load()),
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
        // The ACP surface is the headless surface (ticket #73): the frame
        // replaces the interactive product's base prompt; the context files
        // and skills catalog still append after it.
        base_prompt: Some(crate::headless::FRAME.to_owned()),
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
    // id for this workspace (the project layer included, M4), the effective
    // default current. Absent when no model is configured (an empty select
    // would be a contract violation).
    let (models, default) = server.config.model_options_for(cwd).await;
    let mut result = json!({ "sessionId": session.id });
    let mut context_window = None;
    if !models.is_empty() {
        let current = default.expect("a non-empty model list has a default");
        result["configOptions"] = json!([model_option(&models, &current)]);
        context_window = server.config.context_window_for(cwd, &current).await;
    }
    server
        .registry
        .lock()
        .expect("session registry: no panic while the lock is held")
        .insert(
            session.id.clone(),
            SessionState {
                workspace: workspace.id,
                cwd: cwd.to_owned(),
                mapper: Mapper::default(),
                in_flight: false,
                pending: None,
                saw_stream_end: false,
                saw_interrupted: false,
                cancel_requested: false,
                cancel_active: false,
                context_window,
                task: None,
                last_assistant: None,
                headless: crate::headless::EpisodeState::default(),
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

/// The model-select `configOptions` entry (Harbor's `--model` path): every
/// configured model id, `currentValue` on the effective selection.
fn model_option(models: &[String], current: &str) -> Value {
    let options = models
        .iter()
        .map(|m| json!({ "value": m, "name": m }))
        .collect::<Vec<_>>();
    json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": options,
    })
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
        // A new client prompt starts a fresh headless run (ticket #73).
        begin_headless_run(state, &text);
        state.saw_stream_end = false;
        state.saw_interrupted = false;
        state.cancel_requested = false;
        state.cancel_active = false;
        state.pending = Some(tx);
    }

    if let Err(e) = core.dispatch(Command::MessageSend {
        session: session_id.to_owned(),
        text,
        lane: MessageLane::FollowUp,
    }) {
        // Refused before the turn started: release the lock and answer
        // now, not at a settle that will never come.
        release_turn_lock(server, session_id);
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

/// A new client prompt starts a fresh headless run: the task anchor is
/// this prompt, the episode state restarts, and the last-assistant
/// tracker resets (ticket #73).
fn begin_headless_run(state: &mut crate::sessions::SessionState, text: &str) {
    state.task = Some(text.to_owned());
    state.headless = crate::headless::EpisodeState {
        episode: 1,
        ..Default::default()
    };
    state.last_assistant = None;
}

/// Release the turn lock after a dispatch refusal: a refused send never
/// settles, so the lock is released here instead of at the settle.
fn release_turn_lock(server: &Server, session_id: &str) {
    let mut reg = server
        .registry
        .lock()
        .expect("session registry: no panic while the lock is held");
    if let Some(state) = reg.get_mut(session_id) {
        state.in_flight = false;
        state.pending = None;
    }
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
    // The cancel flag lands before the stop: a settle the abort surfaces
    // as a `System` error still answers `cancelled` (N3).
    {
        let mut reg = server
            .registry
            .lock()
            .expect("session registry: no panic while the lock is held");
        if let Some(state) = reg.get_mut(session_id) {
            state.cancel_requested = true;
        }
    }
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
    let cwd = {
        let reg = server
            .registry
            .lock()
            .expect("session registry: no panic while the lock is held");
        let Some(state) = reg.get(session_id) else {
            drop(reg);
            server.out.send(&transport::error(
                id,
                INVALID_PARAMS,
                &format!("unknown session {session_id:?}"),
            ));
            return;
        };
        state.cwd.clone()
    };
    let core = server.ensure_core().await;
    let value = value.expect("checked above");
    match core.dispatch(Command::SessionSetModel {
        session: session_id.to_owned(),
        model: value.to_owned(),
    }) {
        Ok(_) => {
            // The v1 schema marks `configOptions` required on this response:
            // the updated model select, `currentValue` on the model just set.
            let (models, _default) = server.config.model_options_for(&cwd).await;
            // The window follows the selection (N5): the next turn's
            // `usage_update` sizes against the new model.
            let context_window = server.config.context_window_for(&cwd, value).await;
            {
                let mut reg = server
                    .registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                if let Some(state) = reg.get_mut(session_id) {
                    state.context_window = context_window;
                }
            }
            server.out.send(&transport::result(
                id,
                &json!({ "configOptions": [model_option(&models, value)] }),
            ));
        }
        Err(e) => server
            .out
            .send(&transport::error(id, INTERNAL_ERROR, &protocol_message(&e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text_joins_text_blocks() {
        let text = prompt_text(Some(&json!({
            "prompt": [
                { "type": "text", "text": "first" },
                { "type": "text", "text": "second" }
            ]
        })))
        .expect("text blocks are accepted");
        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn prompt_text_rejects_non_text_blocks() {
        for block in [
            json!({ "type": "image", "data": "x", "mimeType": "image/png" }),
            json!({ "type": "audio", "data": "x", "mimeType": "audio/wav" }),
            json!({ "type": "resource", "resource": { "uri": "file:///x" } }),
        ] {
            let err = prompt_text(Some(&json!({ "prompt": [block] })))
                .expect_err("non-text blocks are rejected");
            assert!(err.contains("unsupported prompt block type"), "{err}");
        }
    }

    #[test]
    fn prompt_text_rejects_missing_and_empty() {
        assert!(prompt_text(None).is_err(), "no params");
        assert!(prompt_text(Some(&json!({}))).is_err(), "no prompt array");
        assert!(
            prompt_text(Some(&json!({ "prompt": [] }))).is_err(),
            "empty array"
        );
        assert!(
            prompt_text(Some(&json!({ "prompt": [{ "type": "text", "text": "" }] }))).is_err(),
            "empty text"
        );
        assert!(
            prompt_text(Some(&json!({ "prompt": [{}] }))).is_err(),
            "block missing type"
        );
    }
}
