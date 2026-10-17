use crate::agent::{
    Arc, AtomicBool, Entry, EntryEventHook, KIND_ASSISTANT, Ordering, TurnEvent, TurnSink,
};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

/// The sink the loop gives the provider: the per-call kill flag (force)
/// or the persistent stop flag (ticket #23) stops the stream (spec §7).
/// It also accumulates the streamed text and re-emits it as the growing
/// assistant entry (ADR-0008's wire-only snapshots), at most one per
/// display frame.
pub(super) struct StreamEntrySink {
    pub(super) kill: Arc<AtomicBool>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) id: String,
    pub(super) parent: Option<String>,
    pub(super) timestamp: u64,
    pub(super) upsert: Option<EntryEventHook>,
    pub(super) text: String,
    pub(super) reasoning: String,
    pub(super) last: Option<Instant>,
}

/// The snapshot cadence: one re-emission per display frame (ADR-0008).
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(16);

impl TurnSink for StreamEntrySink {
    fn event(&mut self, event: TurnEvent) -> bool {
        if self.kill.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        match &event {
            TurnEvent::Text(t) => {
                self.text.push_str(t);
                self.emit_snapshot();
            }
            TurnEvent::Reasoning(t) => {
                self.reasoning.push_str(t);
                self.emit_snapshot();
            }
            // Completed carries no content; the final append's upsert is
            // the stream's last emission.
            TurnEvent::Completed(_) => {}
        }
        true
    }
    fn stop_signal(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(stop_flags(&self.kill, &self.stop))
    }
}

impl StreamEntrySink {
    /// The frame-aligned re-emission: at most one snapshot per window, the
    /// first token opening the card.
    fn emit_snapshot(&mut self) {
        let Some(hook) = &self.upsert else {
            return;
        };
        let now = Instant::now();
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < SNAPSHOT_INTERVAL)
        {
            return;
        }
        self.last = Some(now);
        let entry = Entry {
            id: self.id.clone(),
            parent: self.parent.clone(),
            kind: KIND_ASSISTANT.to_owned(),
            timestamp: self.timestamp,
            payload: tau_protocol::payload::AssistantPayload {
                text: self.text.clone(),
                reasoning: self.reasoning.clone(),
                interrupted: false,
                usage: None,
                calls: vec![],
            }
            .to_value(),
            blob: None,
            first_kept_entry_id: None,
            crc: None,
        };
        hook(&entry);
    }
}
/// The stop-flag poll cadence: a human clicking stop, not a tight race.
const STOP_POLL_MS: u64 = 50;

/// The prefill half of a kill/stop (spec §7): a poll over the flags so the
/// provider tears the in-flight request down before the first stream
/// event arrives.
async fn stop_flags(kill: &AtomicBool, stop: &AtomicBool) {
    loop {
        if kill.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(STOP_POLL_MS)).await;
    }
}
