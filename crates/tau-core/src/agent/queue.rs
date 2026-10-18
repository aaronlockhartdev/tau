use super::{AgentSession, KIND_USER, Lane, Queued};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

impl AgentSession {
    /// Queue `text` on `lane` (shared by `send`/`send_skill`): clears a
    /// prior stop (ticket #23) — a stop means "interrupt current work", not
    /// "never work again"; a Force lane also kills the stream and takes the
    /// head of the queue.
    fn enqueue(&self, text: impl Into<String>, lane: Lane, skill: Option<(String, String)>) {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        self.stop.store(false, Ordering::SeqCst);
        let msg = Queued {
            text: text.into(),
            lane,
            source: None,
            skill,
            entry_id: None,
        };
        if lane == Lane::Force {
            self.kill.store(true, Ordering::SeqCst);
            inner.queue.push_front(msg);
        } else {
            inner.queue.push_back(msg);
        }
    }

    /// Queue a user message on its lane (spec §7). A force kills the
    /// in-flight stream and jumps to the head of the queue; everything else
    /// keeps its position.
    pub fn send(&self, text: impl Into<String>, lane: Lane) {
        self.enqueue(text, lane, None);
    }

    /// A `/skill:` invocation expanded at the app's `message_send` boundary
    /// (ticket #28): `text` is already the expansion template; the payload
    /// carries the skill's identity for the GUI's block.
    pub(crate) fn send_skill(
        &self,
        text: impl Into<String>,
        lane: Lane,
        name: &str,
        location: &str,
    ) {
        self.enqueue(text, lane, Some((name.to_owned(), location.to_owned())));
    }

    /// A child that reports while
    /// the parent's turn is in flight lands on the steering lane, so the
    /// report rides the current turn's next LLM call — temporally correct
    /// where a follow-up would splice it into a later, unrelated turn.
    /// An idle parent degrades to the next turn (the wake starts one).
    pub fn send_notified_steer(&self, text: impl Into<String>, source: String) {
        self.queue_notified(text, Lane::Steering, source);
    }

    fn queue_notified(&self, text: impl Into<String>, lane: Lane, source: String) {
        let mut inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        self.stop.store(false, Ordering::SeqCst);
        let text = text.into();
        let steering = matches!(lane, Lane::Steering);
        // The report's entry is minted now and its upsert fires now — the
        // GUI shows it while the parent's turn is in flight — but the file
        // line is NOT written here: a queue-time write would land ahead of
        // the in-flight assistant entry's line (minted earlier, written when
        // the call returns), inverting file order against id order (#148).
        // The turn loop's dequeue writes the line under the same id, after
        // that line has landed.
        let entry_id = if steering {
            let id = inner.store.mint_id();
            inner.store.upsert_event(
                &id,
                KIND_USER,
                tau_protocol::payload::UserPayload {
                    text: text.clone(),
                    lane: "steering".to_owned(),
                    source: Some(source.clone()),
                    skill: None,
                }
                .to_value(),
            );
            Some(id)
        } else {
            None
        };
        inner.queue.push_back(Queued {
            text,
            lane,
            source: Some(source),
            skill: None,
            entry_id,
        });
    }

    /// A persistent stop (ticket #23): cuts the in-flight stream —
    /// including during the prefill window, before the first token — and
    /// stays set until the next send.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Mark the session closed: a one-way transition that interrupts the
    /// in-flight turn (set by `SessionClose`; unlike `stop`, a new send does
    /// not reset it).
    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// The persistent stop flag (a forwarding seam can share it so one
    /// flag cuts at both the transport mirror and the loop sink).
    pub(crate) fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Whether a queued message is waiting (a parked sub-agent with a
    /// queued message is resumed by it, ADR-0001).
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .queue
            .is_empty()
    }

    /// The queued message at the head of the lane queue — for a
    /// turn-starting send, the one the in-flight turn will deliver first.
    /// The harness uses it to decide whether a failed turn-start still has
    /// a pending message to surface in the queue's projection.
    pub(crate) fn first_pending(&self) -> Option<(String, Lane)> {
        self.inner
            .lock()
            .expect("agent inner: no panic while the lock is held")
            .queue
            .front()
            .map(|q| (q.text.clone(), q.lane))
    }

    /// The session's pending messages as the GUI's queue shows them: a
    /// projection of the loop's queue (the single source of truth), taken
    /// at emit time — the GUI keeps no second ledger of its own.
    pub(crate) fn queued_items(&self) -> Vec<tau_protocol::snapshot::QueuedItem> {
        let inner = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        inner
            .queue
            .iter()
            .map(|q| tau_protocol::snapshot::QueuedItem {
                text: q.text.clone(),
                lane: crate::harness::lane_to_message_lane(q.lane),
                source: q.source.clone(),
            })
            .collect()
    }
}
