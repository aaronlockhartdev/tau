//! 25 ms delta coalescing (spec §8, user-specified cadence): outgoing
//! stream deltas are batched and flushed on a timer; everything else is
//! flushed immediately, so a stream end is never held up behind a window.
//! The clock is injected — the cadence is testable without sleeping.

use crate::Event;

pub struct Coalescer {
    window_ms: u64,
    pending: Vec<Event>,
    due_at: Option<u64>,
}

impl Coalescer {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            pending: Vec::new(),
            due_at: None,
        }
    }

    /// Queue an event. The first delta after a flush arms the window;
    /// further deltas (any call) merge into it and wait for the same
    /// deadline. Non-deltas force an immediate flush.
    pub fn push(&mut self, event: Event, now: u64) {
        match &event {
            Event::StreamDelta {
                workspace,
                session,
                call_id,
                text,
                reasoning,
            } => {
                if let Some(last) = self
                    .pending
                    .iter_mut()
                    .rev()
                    .find(|p| matches!(p, Event::StreamDelta { workspace: w, session: s, call_id: c, .. } if w == workspace && s == session && c == call_id))
                {
                    if let Event::StreamDelta {
                        text: t,
                        reasoning: r,
                        ..
                    } = last
                    {
                        t.push_str(text);
                        *r = merge_reasoning(std::mem::take(r), reasoning.clone());
                    }
                } else {
                    self.pending.push(event);
                }
                if self.due_at.is_none() {
                    self.due_at = Some(now + self.window_ms);
                }
            }
            _ => {
                self.pending.push(event);
                self.due_at = Some(now);
            }
        }
    }

    /// Take everything that is due at `now` (in arrival order).
    pub fn take(&mut self, now: u64) -> Vec<Event> {
        match self.due_at {
            Some(due) if due <= now => {
                self.due_at = None;
                std::mem::take(&mut self.pending)
            }
            _ => Vec::new(),
        }
    }
}

fn merge_reasoning(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            a.push_str(&b);
            Some(a)
        }
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(call: &str, text: &str) -> Event {
        Event::StreamDelta {
            workspace: "w".into(),
            session: "s".into(),
            call_id: call.into(),
            text: text.into(),
            reasoning: None,
        }
    }

    fn end(call: &str) -> Event {
        Event::StreamEnd {
            workspace: "w".into(),
            session: "s".into(),
            call_id: call.into(),
            interrupted: false,
            usage: None,
        }
    }

    #[test]
    fn a_burst_of_deltas_flushes_once_at_the_window() {
        let mut c = Coalescer::new(25);
        c.push(delta("c1", "a"), 0);
        c.push(delta("c1", "b"), 5);
        c.push(delta("c1", "c"), 10);
        assert_eq!(c.take(24), Vec::<Event>::new());
        let batch = c.take(25);
        assert_eq!(batch.len(), 1);
        match &batch[0] {
            Event::StreamDelta { text, .. } => assert_eq!(text, "abc"),
            other => panic!("expected one merged delta, got {other:?}"),
        }
        // The next delta arms a fresh window.
        c.push(delta("c1", "d"), 26);
        assert_eq!(c.take(40), Vec::<Event>::new());
        let batch = c.take(51);
        assert_eq!(batch.len(), 1);
        match &batch[0] {
            Event::StreamDelta { text, .. } => assert_eq!(text, "d"),
            other => panic!("expected the late delta, got {other:?}"),
        }
    }

    #[test]
    fn a_non_delta_flushes_early() {
        let mut c = Coalescer::new(25);
        c.push(delta("c1", "partial"), 0);
        c.push(end("c1"), 5);
        let batch = c.take(5);
        assert_eq!(batch.len(), 2);
        assert!(matches!(&batch[0], Event::StreamDelta { .. }));
        assert!(matches!(&batch[1], Event::StreamEnd { .. }));
    }

    #[test]
    fn interleaved_calls_keep_their_streams_separate() {
        let mut c = Coalescer::new(25);
        c.push(delta("c1", "a"), 0);
        c.push(delta("c2", "x"), 2);
        c.push(delta("c1", "b"), 4);
        let batch = c.take(25);
        assert_eq!(batch.len(), 2);
        match (&batch[0], &batch[1]) {
            (
                Event::StreamDelta { call_id, text, .. },
                Event::StreamDelta {
                    call_id: c2,
                    text: t2,
                    ..
                },
            ) => {
                assert_eq!(call_id, "c1");
                assert_eq!(text, "ab");
                assert_eq!(c2, "c2");
                assert_eq!(t2, "x");
            }
            other => panic!("expected two per-call deltas, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_deltas_merge_into_their_batch() {
        let mut c = Coalescer::new(25);
        c.push(
            Event::StreamDelta {
                workspace: "w".into(),
                session: "s".into(),
                call_id: "c1".into(),
                text: String::new(),
                reasoning: Some("thi".into()),
            },
            0,
        );
        c.push(
            Event::StreamDelta {
                workspace: "w".into(),
                session: "s".into(),
                call_id: "c1".into(),
                text: "out".into(),
                reasoning: Some("nking".into()),
            },
            3,
        );
        let batch = c.take(25);
        match &batch[0] {
            Event::StreamDelta {
                text, reasoning, ..
            } => {
                assert_eq!(text, "out");
                assert_eq!(reasoning.as_deref(), Some("thinking"));
            }
            other => panic!("expected a merged delta, got {other:?}"),
        }
    }
}
