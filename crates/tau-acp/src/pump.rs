//! The dedicated event pump: the single consumer of `Core::events()`,
//! mapping core events to ACP `session/update` notifications and driving
//! the settle rule. At the 16 ms snapshot cadence (ADR-0008) the bounded
//! pipe is effectively lossless for a consumer whose only job is this; a
//! stall long enough to drop events surfaces as a `System` overflow error,
//! which settles the affected turn (the journal-resync repair is the
//! post-v0 follow-up, research doc §5.1).

use std::sync::Arc;

use crate::sessions::{self, Outcome, Registry};
use crate::transport::{self, Out};
use serde_json::json;
use tau_core::harness::Core;
use tau_protocol::{Event, SystemEventKind};

pub async fn run(core: Arc<Core>, out: Out, registry: Arc<Registry>) {
    let mut rx = core.events();
    while let Some(event) = rx.recv().await {
        handle(&event, &out, &registry);
    }
}

fn handle(event: &Event, out: &Out, registry: &Registry) {
    match event {
        Event::EntryUpsert { session, entry, .. } => {
            let updates = {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
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
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                sessions::settle(&mut guard, session, outcome);
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
