//! The dedicated event pump: the single consumer of `Core::events()`,
//! mapping core events to ACP `session/update` notifications and driving
//! the settle rule. At the 16 ms snapshot cadence (ADR-0008) the bounded
//! pipe is effectively lossless for a consumer whose only job is this; a
//! stall long enough to drop events surfaces as a `System` overflow error,
//! which settles the affected turn (the journal-resync repair is the
//! post-v0 follow-up, research doc §5.1).

use std::collections::HashMap;
use std::sync::Arc;

use crate::sessions::{self, Outcome, Registry};
use crate::transport::{self, Out};
use serde_json::json;
use tau_core::harness::Core;
use tau_protocol::snapshot::ViewEntry;
use tau_protocol::{Command, Event, MessageLane, SystemEventKind};

pub async fn run(core: Arc<Core>, out: Out, registry: Arc<Registry>) {
    let params = crate::headless::Params::from_env();
    let mut rx = core.events();
    while let Some(event) = rx.recv().await {
        handle(&event, &core, &out, &registry, &params);
    }
}

fn handle(
    event: &Event,
    core: &Arc<Core>,
    out: &Out,
    registry: &Registry,
    params: &crate::headless::Params,
) {
    match event {
        Event::EntryUpsert { session, entry, .. } => {
            let updates = {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                track_last_assistant(&mut guard, session, entry);
                guard
                    .get_mut(session)
                    .map(|s| s.mapper.on_entry(entry))
                    .unwrap_or_default()
            };
            for update in updates {
                out.send(&transport::session_update(session, &update));
            }
        }
        Event::StreamEnd {
            session,
            interrupted,
            usage,
            ..
        } => {
            let usage_update = {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                guard.get_mut(session).and_then(|s| {
                    // Finalizes the oldest in-flight assistant entry; a
                    // StreamEnd alone never settles (the empty Queue is the
                    // settle signal).
                    s.mapper.on_stream_end();
                    s.saw_stream_end = true;
                    if *interrupted {
                        s.saw_interrupted = true;
                    }
                    // N5: per-call usage rides the finalize, only when the
                    // session's model declares a context window — the
                    // schema's `size` is required and must not be guessed.
                    // Built under the lock, sent after (the writer lock is
                    // separate).
                    usage.and_then(|u| {
                        s.context_window.map(|size| {
                            json!({
                                "sessionUpdate": "usage_update",
                                "used": u.input_tokens + u.output_tokens,
                                "size": size,
                            })
                        })
                    })
                })
            };
            if let Some(update) = usage_update {
                out.send(&transport::session_update(session, &update));
            }
        }
        Event::Queue { session, items, .. } => {
            let outcome = {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                guard.get_mut(session).and_then(|s| {
                    if s.saw_stream_end && items.is_empty() {
                        // A requested cancel wins over a clean settle: the
                        // spec answers `cancelled` even when the abort
                        // surfaced without the interrupted flag (N3).
                        Some(if s.saw_interrupted || s.cancel_requested {
                            Outcome::Cancelled
                        } else {
                            Outcome::EndTurn
                        })
                    } else if items.is_empty() && s.cancel_requested {
                        // A cancel with an empty queue settles even without a
                        // `StreamEnd`: a call aborted before its first event
                        // leaves none behind (the core records calls at first
                        // event), and the spec MUSTs the `cancelled` answer.
                        Some(Outcome::Cancelled)
                    } else {
                        None
                    }
                })
            };
            if let Some(outcome) = outcome {
                settle_headless(core, out, registry, params, session, outcome);
            }
        }
        // A session-scoped system error kills the turn: the prompt gets a
        // JSON-RPC error (v1 has no `error` stop reason).
        Event::System {
            session: Some(session),
            kind: SystemEventKind::Error { message },
            ..
        } => {
            let mut guard = registry
                .lock()
                .expect("session registry: no panic while the lock is held");
            let outcome = guard.get_mut(session).map(|s| {
                // A cancel in flight turns the error into the spec-mandated
                // `cancelled` response (N3).
                if s.cancel_requested {
                    Outcome::Cancelled
                } else {
                    Outcome::Error(message.clone())
                }
            });
            if let Some(outcome) = outcome {
                sessions::settle(&mut guard, session, outcome);
            }
        }
        // Workspace-level and non-turn events (skills, file tree, subagent
        // children): no ACP v0 surface.
        _ => {}
    }
}

/// The headless settle (ticket #73): an `EndTurn` without a completion
/// declaration is not terminal — the episode loop answers it with a
/// task-anchored continuation prompt. Cancellation and errors are terminal
/// in every mode.
fn settle_headless(
    core: &Arc<Core>,
    out: &Out,
    registry: &Registry,
    params: &crate::headless::Params,
    session: &str,
    outcome: Outcome,
) {
    let _ = out; // the response rides the pending oneshot, not the pump
    let decision = match &outcome {
        Outcome::EndTurn => {
            let mut guard = registry
                .lock()
                .expect("session registry: no panic while the lock is held");
            guard
                .get_mut(session)
                .map_or(crate::headless::Decision::Done(Outcome::EndTurn), |s| {
                    let (text, task) = match (&s.last_assistant, &s.task) {
                        (Some(t), Some(task)) => (t.clone(), task.clone()),
                        _ => (String::new(), String::new()),
                    };
                    crate::headless::decide(&mut s.headless, params, &task, &text)
                })
        }
        _ => crate::headless::Decision::Done(outcome),
    };
    match decision {
        crate::headless::Decision::Done(outcome) => {
            let mut guard = registry
                .lock()
                .expect("session registry: no panic while the lock is held");
            sessions::settle(&mut guard, session, outcome);
        }
        crate::headless::Decision::Continue(nudge) => {
            // Episode N+1: the turn flags reset for the new turn; the
            // prompt response stays pending until a real end.
            let cancelled = {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                // A cancel landing between the settle decision and this
                // reset would be wiped: read it before the reset and honor
                // it after.
                let cancelled = guard.get(session).is_some_and(|s| s.cancel_requested);
                if let Some(s) = guard.get_mut(session) {
                    s.saw_stream_end = false;
                    s.saw_interrupted = false;
                    s.cancel_requested = false;
                    s.last_assistant = None;
                }
                cancelled
            };
            if cancelled {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                sessions::settle(&mut guard, session, Outcome::Cancelled);
            }
            if !cancelled
                && let Err(e) = core.dispatch(Command::MessageSend {
                    session: session.to_owned(),
                    text: nudge,
                    lane: MessageLane::FollowUp,
                })
            {
                // Refused before the turn started: settle now — never loop
                // on a refused send.
                let message = match &e {
                    tau_protocol::ProtocolError::Unsupported { message }
                    | tau_protocol::ProtocolError::Other { message } => message.clone(),
                    tau_protocol::ProtocolError::NotFound { what } => what.clone(),
                };
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                sessions::settle(&mut guard, session, Outcome::Error(message));
            }
        }
    }
}

/// The headless settle reads the turn's final assistant text for the
/// completion marker: remember the last assistant upsert per session.
fn track_last_assistant(
    sessions: &mut HashMap<String, crate::sessions::SessionState>,
    session: &str,
    entry: &ViewEntry,
) {
    if entry.kind != "assistant" {
        return;
    }
    let Some(s) = sessions.get_mut(session) else {
        return;
    };
    let text = entry
        .payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    s.last_assistant = Some(text);
}
