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
                    // Finalizes every in-flight assistant entry (the end is
                    // per-turn, #82); a StreamEnd alone never settles (the
                    // empty Queue is the settle signal).
                    s.mapper.on_stream_end();
                    s.saw_stream_end = true;
                    // A cancel may rewrite this turn's outcome only if it
                    // predates the stream end; one landing in the post-turn
                    // window (settle pending) must not (ticket #78).
                    s.cancel_active = s.cancel_requested;
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
                guard
                    .get_mut(session)
                    .and_then(|s| queue_settle(s, items.is_empty()))
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

/// The `Queue` settle rule (extracted for the ticket #78 tests): after the
/// first `StreamEnd`, an empty queue settles the turn; a cancel that
/// predates the stream end — or an interrupted stream — answers `cancelled`
/// (N3). A cancel that lands after the stream end (the post-turn window)
/// must not rewrite a completed turn's outcome.
fn queue_settle(s: &crate::sessions::SessionState, queue_empty: bool) -> Option<Outcome> {
    if s.saw_stream_end && queue_empty {
        Some(if s.saw_interrupted || s.cancel_active {
            Outcome::Cancelled
        } else {
            Outcome::EndTurn
        })
    } else if queue_empty && s.cancel_requested {
        // A cancel with an empty queue settles even without a `StreamEnd`:
        // a call aborted before its first event leaves none behind (the
        // core records calls at first event), and the spec MUSTs the
        // `cancelled` answer.
        Some(Outcome::Cancelled)
    } else {
        None
    }
}

/// The headless settle (ticket #73): an `EndTurn` without a completion
/// declaration is not terminal — the episode loop answers it with a
/// task-anchored continuation prompt. A client cancel that predates the
/// turn's stream end, and errors, are terminal; a cancel landing in the
/// post-turn window answers the completed outcome, not `cancelled`
/// (ticket #78).
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
                    s.cancel_active = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn settled_turn() -> crate::sessions::SessionState {
        crate::sessions::SessionState {
            workspace: "w".into(),
            cwd: "/w".into(),
            mapper: crate::mapper::Mapper::default(),
            in_flight: true,
            pending: None,
            saw_stream_end: true,
            saw_interrupted: false,
            cancel_requested: false,
            cancel_active: false,
            context_window: None,
            task: Some("task".into()),
            last_assistant: None,
            headless: crate::headless::EpisodeState::default(),
        }
    }

    // Ticket #78: a cancel that lands after the stream ended (the post-turn
    // window) must not rewrite a completed turn's outcome.
    #[test]
    fn a_late_cancel_keeps_the_completed_outcome() {
        let mut s = settled_turn();
        s.cancel_requested = true; // arrived after the StreamEnd snapshot
        assert_eq!(queue_settle(&s, true), Some(Outcome::EndTurn));
    }

    // A cancel that predates the stream end still answers `cancelled` (N3).
    #[test]
    fn an_early_cancel_still_answers_cancelled() {
        let mut s = settled_turn();
        s.cancel_requested = true;
        s.cancel_active = true; // the StreamEnd snapshot saw it
        assert_eq!(queue_settle(&s, true), Some(Outcome::Cancelled));
    }

    #[test]
    fn a_clean_settle_is_an_end_turn() {
        assert_eq!(queue_settle(&settled_turn(), true), Some(Outcome::EndTurn));
    }

    // A cancel with no `StreamEnd` at all (aborted before the first event)
    // settles `cancelled`.
    #[test]
    fn a_cancel_without_stream_end_answers_cancelled() {
        let mut s = settled_turn();
        s.saw_stream_end = false;
        s.cancel_requested = true;
        assert_eq!(queue_settle(&s, true), Some(Outcome::Cancelled));
    }

    // A non-empty queue never settles, cancel or not.
    #[test]
    fn a_non_empty_queue_never_settles() {
        let mut s = settled_turn();
        s.cancel_requested = true;
        assert_eq!(queue_settle(&s, false), None);
    }
}
