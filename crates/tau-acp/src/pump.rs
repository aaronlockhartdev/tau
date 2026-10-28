//! The dedicated event pump: the single consumer of `Core::events()`,
//! mapping core events to ACP `session/update` notifications and driving
//! the settle rule. At the display's frame cadence (ADR-0008) the bounded pipe
//! is effectively lossless for a consumer whose only job is this; a stall long
//! enough to drop events surfaces as a `System` overflow error, which settles
//! the affected turn (the journal-resync repair is the post-v0 follow-up,
//! research doc §5.1). A dropped stream frame is the one loss the stream
//! survives: its entry's record carries the text (#303).

use std::collections::HashMap;
use std::sync::Arc;

use crate::sessions::{self, Outcome, Registry};
use crate::transport::{self, Out};
use serde_json::{Value, json};
use tau_core::harness::Core;
use tau_protocol::snapshot::ViewEntry;
use tau_protocol::{Command, EntryKind, Event, MessageLane, SystemEventKind};

/// The four values every pump handler needs, named once: a future pump
/// dependency (a clock, a policy) adds a field, not a parameter to `handle`
/// and `settle_headless`.
struct PumpCtx<'a> {
    core: &'a Arc<Core>,
    out: &'a Out,
    registry: &'a Registry,
    params: &'a crate::headless::Params,
}

pub async fn run(core: Arc<Core>, out: Out, registry: Arc<Registry>) {
    let params = crate::headless::Params::from_env();
    let ctx = PumpCtx {
        core: &core,
        out: &out,
        registry: registry.as_ref(),
        params: &params,
    };
    let mut rx = core.events();
    while let Some(event) = rx.recv().await {
        handle(&ctx, &event);
    }
}

/// Forward one core delta (#303) to the ACP client: the producer cut the text
/// to its new characters, so the mapper's chunks ride out as they are. The
/// `let`-`else` is the pattern the pump's own arm already matched; the 100-line
/// ceiling on `handle` rules out destructuring it there.
fn forward_delta(ctx: &PumpCtx, event: &Event) {
    let Event::StreamDelta {
        session,
        id,
        seq,
        text,
        reasoning,
        ..
    } = event
    else {
        return;
    };
    let chunks = delta_chunks(ctx.registry, session, id, *seq, text, reasoning);
    for update in chunks {
        ctx.out.send(&transport::session_update(session, &update));
    }
}

/// The ACP chunks a core delta becomes (#303): the producer cut the delta, so
/// the mapper forwards it and the counters it advances keep the entry's file
/// line from repeating the text. A session the registry does not know owes
/// nothing.
fn delta_chunks(
    registry: &Registry,
    session: &str,
    id: &str,
    seq: u64,
    text: &str,
    reasoning: &str,
) -> Vec<Value> {
    registry
        .lock()
        .get_mut(session)
        .map(|s| s.mapper.on_delta(id, seq, text, reasoning))
        .unwrap_or_default()
}

fn handle(ctx: &PumpCtx, event: &Event) {
    let PumpCtx { out, registry, .. } = ctx;
    match event {
        Event::EntryUpsert { session, entry, .. } => {
            let updates = {
                let mut guard = registry.lock();
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
        // The streaming delta (#303): the chunk is already cut, so the pump
        // forwards it and never sees the accumulation.
        Event::StreamDelta { .. } => forward_delta(ctx, event),
        Event::StreamEnd {
            session,
            interrupted,
            usage,
            ..
        } => {
            let usage_update = {
                let mut guard = registry.lock();
                guard.get_mut(session).and_then(|s| {
                    // Finalizes every in-flight assistant entry (the end is
                    // per-turn, #82); a StreamEnd alone never settles (the
                    // empty Queue is the settle signal).
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
            let (outcome, yolo) = {
                let mut guard = registry.lock();
                guard.get_mut(session).map_or((None, false), |s| {
                    (queue_settle(s, items.is_empty()), s.yolo)
                })
            };
            if let Some(outcome) = outcome {
                if yolo {
                    settle_headless(ctx, session, outcome);
                } else {
                    let mut guard = registry.lock();
                    sessions::settle(&mut guard, session, outcome);
                }
            }
        }
        // A session-scoped system error kills the turn: the prompt gets a
        // JSON-RPC error (v1 has no `error` stop reason).
        Event::System {
            session: Some(session),
            kind: SystemEventKind::Error { message },
            ..
        } => {
            let mut guard = registry.lock();
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

/// The `Queue` settle rule (extracted for the ticket #78 tests), strict
/// cancel (ticket #84): a cancel that landed for the turn — before the
/// stream ended, in the post-turn window, or on a call aborted before its
/// first event — answers `cancelled` (the spec MUSTs the response, N3);
/// otherwise an ended stream with an empty queue ends the turn.
fn queue_settle(s: &crate::sessions::SessionState, queue_empty: bool) -> Option<Outcome> {
    if queue_empty && s.cancel_requested {
        return Some(Outcome::Cancelled);
    }
    if s.saw_stream_end && queue_empty {
        Some(Outcome::EndTurn)
    } else {
        None
    }
}

/// The headless settle (ticket #73): an `EndTurn` without a completion
/// declaration is not terminal — the episode loop answers it with a
/// task-anchored continuation prompt. A client cancel and errors are
/// terminal (strict cancel, ticket #84).
fn settle_headless(ctx: &PumpCtx, session: &str, outcome: Outcome) {
    let PumpCtx {
        core,
        out,
        registry,
        params,
    } = ctx;
    let _ = out; // the response rides the pending oneshot, not the pump
    let decision = match &outcome {
        Outcome::EndTurn => {
            let mut guard = registry.lock();
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
            let mut guard = registry.lock();
            sessions::settle(&mut guard, session, outcome);
        }
        crate::headless::Decision::Continue(nudge) => {
            // Episode N+1: the turn flags reset for the new turn; the
            // prompt response stays pending until a real end.
            let cancelled = {
                let mut guard = registry.lock();
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
                let mut guard = registry.lock();
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
                let message = e.to_string();
                let mut guard = registry.lock();
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
    if entry.kind != EntryKind::Assistant {
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
            yolo: false,
            context_window: None,
            task: Some("task".into()),
            last_assistant: None,
            headless: crate::headless::EpisodeState::default(),
        }
    }

    // Ticket #84 (strict reading, ACP v1 prompt-turn Cancellation): a
    // cancel that lands after the stream ended but before the prompt
    // response is written answers `cancelled` — the settle reads the live
    // `cancel_requested` flag, not a stream-end snapshot.
    #[test]
    fn a_late_cancel_answers_cancelled() {
        let mut s = settled_turn();
        s.cancel_requested = true; // arrived after the StreamEnd
        assert_eq!(queue_settle(&s, true), Some(Outcome::Cancelled));
    }

    // A cancel that predates the stream end still answers `cancelled` (N3).
    #[test]
    fn an_early_cancel_still_answers_cancelled() {
        let mut s = settled_turn();
        s.cancel_requested = true;
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
