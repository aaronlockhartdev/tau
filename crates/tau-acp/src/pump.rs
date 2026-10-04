//! The dedicated event pump: the single consumer of `Core::events()`,
//! mapping core events to ACP `session/update` notifications and driving
//! the settle rule. At the 16 ms snapshot cadence (ADR-0008) the bounded
//! pipe is effectively lossless for a consumer whose only job is this; a
//! stall long enough to drop events surfaces as a `System` overflow error,
//! which settles the affected turn (the journal-resync repair is the
//! post-v0 follow-up, research doc §5.1).

use std::sync::Arc;

use tau_core::harness::Core;
use tau_protocol::{Event, SystemEventKind};

use crate::sessions::{self, Outcome, Registry};
use crate::transport::{self, Out};

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
            ..
        } => {
            // Finalizes the oldest in-flight assistant entry; a StreamEnd
            // alone never settles (the empty Queue is the settle signal).
            let mut guard = registry
                .lock()
                .expect("session registry: no panic while the lock is held");
            if let Some(s) = guard.get_mut(session) {
                s.mapper.on_stream_end();
                s.saw_stream_end = true;
                if *interrupted {
                    s.saw_interrupted = true;
                }
            }
        }
        Event::Queue { session, items, .. } => {
            let outcome = {
                let mut guard = registry
                    .lock()
                    .expect("session registry: no panic while the lock is held");
                guard.get_mut(session).and_then(|s| {
                    if s.saw_stream_end && items.is_empty() {
                        Some(if s.saw_interrupted {
                            Outcome::Cancelled
                        } else {
                            Outcome::EndTurn
                        })
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
            sessions::settle(&mut guard, session, Outcome::Error(message.clone()));
        }
        // Workspace-level and non-turn events (skills, file tree, subagent
        // children): no ACP v0 surface.
        _ => {}
    }
}
