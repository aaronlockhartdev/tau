//! The ACP session methods: the per-method handlers `dispatch_line` routes to
//! (the ACP session surface). Split from `server.rs` at the 500-LOC soft limit
//! (#76) — the server struct, the stdin loop, and the method routing stay
//! there; this is the method bodies.

use serde_json::{Value, json};
use tau_protocol::{Command, CommandOutput, MessageLane};
use tokio::sync::oneshot;

use super::Server;
use crate::mapper::Mapper;
use crate::sessions::{Outcome, SessionState};
use crate::transport::{self, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, Rpc};

pub(super) async fn session_new(server: &Server, rpc: &Rpc) {
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
    let workspace = match server
        .dispatch(
            id,
            Command::WorkspaceOpen {
                cwd: cwd.to_owned(),
            },
        )
        .await
    {
        Some(CommandOutput::Workspace { workspace }) => workspace,
        Some(_) => unreachable!("dispatch routes the arm"),
        None => return,
    };
    let session = match server
        .dispatch(
            id,
            Command::SessionNew {
                workspace: workspace.id.clone(),
                title: None,
                // The ACP surface is the headless surface (ticket #73): the
                // frame replaces the interactive product's base prompt; the
                // context files and skills catalog still append after it.
                base_prompt: Some(crate::headless::FRAME.to_owned()),
            },
        )
        .await
    {
        Some(CommandOutput::Session { session }) => session,
        Some(_) => unreachable!("dispatch routes the arm"),
        None => return,
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

pub(super) async fn session_prompt(server: &Server, rpc: &Rpc) {
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
            .send(&transport::error(id, INTERNAL_ERROR, &e.to_string()));
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

pub(super) async fn session_cancel(server: &Server, rpc: &Rpc) {
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
        tracing::warn!("acp: cancel for {session_id}: {}", e);
    }
    // The in-flight prompt (if any) answers `stopReason: "cancelled"` when
    // the interrupted turn settles.
}

pub(super) async fn set_config_option(server: &Server, rpc: &Rpc) {
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
    let value = value.expect("checked above");
    if server
        .dispatch(
            id,
            Command::SessionSetModel {
                session: session_id.to_owned(),
                model: value.to_owned(),
            },
        )
        .await
        .is_some()
    {
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
