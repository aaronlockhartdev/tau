use crate::agent::{Arc, AtomicBool, Ordering, StreamDelta, StreamDeltaHook, TurnEvent, TurnSink};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

/// The sink the loop gives the provider: the per-call kill flag (force)
/// or the persistent stop flag (ticket #23) stops the stream (spec §7).
/// It accumulates the streamed text and streams it to the live view as
/// frame-aligned deltas, each carrying only the characters appended since the
/// last (#303); the accumulation itself lands once, as the committed entry.
pub(super) struct StreamEntrySink {
    pub(super) kill: Arc<AtomicBool>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) id: String,
    /// The live growth sink; `None` = no live view, and the committed entry is
    /// the first sight of the text.
    pub(super) delta: Option<StreamDeltaHook>,
    pub(super) text: String,
    pub(super) reasoning: String,
    /// Bytes of `text` handed to deltas so far; the next delta starts here. The
    /// mark is always a previous `len()` of this same accumulator, so it lands
    /// on a char boundary.
    pub(super) sent: usize,
    /// Bytes of `reasoning` handed to deltas so far.
    pub(super) sent_reasoning: usize,
    /// Deltas emitted for this entry, from 1; the consumer applies each seq
    /// once, in order, so a repeat is dropped and a gap is visible.
    pub(super) seq: u64,
    pub(super) last: Option<Instant>,
    /// The frame window between deltas: production emits at the display's
    /// cadence, a test can earn a flush on every event.
    pub(super) interval: Duration,
}

/// The emission cadence: one delta per display frame, ADR-0008's frame-aligned
/// streaming. #303 changed the payload, not the cadence.
pub(super) const STREAM_INTERVAL: Duration = Duration::from_millis(16);

impl StreamEntrySink {
    /// The sink one LLM call streams into: an empty accumulation at the
    /// display's cadence (ADR-0008), the call's `kill` and `stop` flags, and
    /// the live view's delta hook — `None` when no view watches the session,
    /// in which case the committed entry is the first sight of the text.
    pub(super) fn new(
        id: String,
        kill: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        delta: Option<StreamDeltaHook>,
    ) -> Self {
        Self {
            kill,
            stop,
            id,
            delta,
            text: String::new(),
            reasoning: String::new(),
            sent: 0,
            sent_reasoning: 0,
            seq: 0,
            last: None,
            interval: STREAM_INTERVAL,
        }
    }
}

impl TurnSink for StreamEntrySink {
    fn event(&mut self, event: TurnEvent) -> bool {
        if self.kill.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            return false;
        }
        match &event {
            TurnEvent::Text(t) => {
                self.text.push_str(t);
                self.emit_delta();
            }
            TurnEvent::Reasoning(t) => {
                self.reasoning.push_str(t);
                self.emit_delta();
            }
            // Completed carries no content; the final append is the stream's
            // last emission.
            TurnEvent::Completed(_, _) => {}
        }
        true
    }
    fn stop_signal(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(stop_flags(&self.kill, &self.stop))
    }
}

impl StreamEntrySink {
    /// The frame-aligned emission: at most one delta per window, the first
    /// token opening the card. A flush hands over the suffix of each
    /// accumulator, so a turn that streams n characters copies n bytes; the
    /// shape this replaces copied the whole accumulation on every flush, which
    /// is n²/16ms bytes per turn.
    fn emit_delta(&mut self) {
        let Some(hook) = &self.delta else {
            return;
        };
        let now = Instant::now();
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < self.interval)
        {
            return;
        }
        if self.text.len() == self.sent && self.reasoning.len() == self.sent_reasoning {
            return;
        }
        self.last = Some(now);
        self.seq += 1;
        let delta = StreamDelta {
            id: self.id.clone(),
            seq: self.seq,
            text: self
                .text
                .get(self.sent..)
                .expect("sent is a length the accumulation already reached, so a char boundary")
                .to_owned(),
            reasoning: self
                .reasoning
                .get(self.sent_reasoning..)
                .expect("sent_reasoning is a length the accumulation already reached, so a char boundary")
                .to_owned(),
        };
        self.sent = self.text.len();
        self.sent_reasoning = self.reasoning.len();
        hook(&delta);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Usage;
    use std::fmt::Write;
    use std::sync::Mutex;

    /// What the sink handed the live view: every delta, and the bytes it copied
    /// to produce them — #303's measured quantity.
    #[derive(Default)]
    struct View {
        deltas: Vec<StreamDelta>,
        copied: usize,
    }

    /// The frame window is a field, so a test can earn a flush on every event;
    /// `StreamEntrySink::new` is what a turn builds, and it sets the cadence.
    fn sink(view: Arc<Mutex<View>>) -> StreamEntrySink {
        sink_at(view, Duration::ZERO)
    }

    fn sink_at(view: Arc<Mutex<View>>, interval: Duration) -> StreamEntrySink {
        let mut sink = StreamEntrySink::new(
            "a1".into(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Some(Arc::new(move |delta: &StreamDelta| {
                let mut seen = view.lock().expect("the view collector does not panic");
                seen.copied += delta.text.len() + delta.reasoning.len();
                seen.deltas.push(delta.clone());
            })),
        );
        sink.interval = interval;
        sink
    }

    /// The wiring a turn gets, straight from the constructor: the display's
    /// cadence, which is the re-render ceiling ADR-0008 sets.
    #[test]
    fn a_turns_sink_streams_at_the_frame_cadence() {
        let sink = StreamEntrySink::new(
            "a1".into(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            None,
        );
        assert_eq!(
            sink.interval,
            Duration::from_millis(16),
            "ADR-0008: at most one delta per display frame"
        );
    }

    /// A frame that carries no new characters owes no delta: a stream that
    /// stops growing stops emitting, so an idle frame costs the view nothing.
    #[test]
    fn a_frame_that_grows_nothing_emits_nothing() {
        let view = Arc::new(Mutex::new(View::default()));
        let mut sink = sink(Arc::clone(&view));
        sink.event(TurnEvent::Text("one".into()));
        sink.event(TurnEvent::Text(String::new()));
        sink.event(TurnEvent::Reasoning(String::new()));
        let seen = view.lock().expect("the view collector does not panic");
        assert_eq!(seen.deltas.len(), 1);
        assert_eq!(seen.deltas[0].text, "one");
    }
    fn stream(sink: &mut StreamEntrySink, rounds: usize) {
        for i in 0..rounds {
            sink.event(TurnEvent::Text(format!("{i:04} ")));
        }
    }

    /// What `rounds` chunks of `<prefix><i> ` add up to.
    fn stream_text(prefix: &str, rounds: usize) -> String {
        let mut out = String::new();
        for i in 0..rounds {
            write!(&mut out, "{prefix}{i} ").expect("writing into a String never fails");
        }
        out
    }

    /// A turn with no live view streams: the accumulation grows, no delta is
    /// minted, and the provider is never refused.
    #[test]
    fn a_stream_with_no_live_view_still_accumulates() {
        let mut sink = sink(Arc::new(Mutex::new(View::default())));
        sink.delta = None;
        assert!(sink.event(TurnEvent::Text("hello".into())));
        assert_eq!(sink.text, "hello");
        assert_eq!(sink.seq, 0);
    }

    /// The deltas tile the stream: in order, seq contiguous from 1, no gap and
    /// no overlap, every character of text and of reasoning arriving, and no
    /// delta carrying nothing.
    #[test]
    fn the_deltas_tile_the_stream() {
        let view = Arc::new(Mutex::new(View::default()));
        let mut sink = sink(Arc::clone(&view));
        for i in 0..20 {
            sink.event(TurnEvent::Text(format!("t{i} ")));
            sink.event(TurnEvent::Reasoning(format!("r{i} ")));
        }
        // The end of a stream is not a growth of it: the completion frame owes
        // no delta, so the run stays at one per growth.
        sink.event(TurnEvent::Completed(Usage::default(), None));
        let seen = view.lock().expect("the view collector does not panic");
        // One flush per growth: every text event and every reasoning event
        // grows the accumulation, and each growth is one frame.
        assert_eq!(seen.deltas.len(), 40);
        let seqs: Vec<u64> = seen.deltas.iter().map(|d| d.seq).collect();
        assert_eq!(seqs, (1..=40).collect::<Vec<u64>>());
        let text: String = seen.deltas.iter().map(|d| d.text.as_str()).collect();
        let reasoning: String = seen.deltas.iter().map(|d| d.reasoning.as_str()).collect();
        assert_eq!(text, stream_text("t", 20));
        assert_eq!(reasoning, stream_text("r", 20));
        assert!(
            seen.deltas
                .iter()
                .all(|d| !d.text.is_empty() || !d.reasoning.is_empty())
        );
        assert!(seen.deltas.iter().all(|d| d.id == "a1"));
    }

    /// The bound #303 exists to buy: 20 flushes of a 100-character stream
    /// copy 100 bytes. The shape this replaces copied the whole accumulation
    /// on every flush — 1050 bytes for the same stream (5 × (1+2+…+20)).
    #[test]
    fn a_flush_copies_the_new_characters_not_the_accumulation() {
        let view = Arc::new(Mutex::new(View::default()));
        let mut sink = sink(Arc::clone(&view));
        stream(&mut sink, 20);
        let streamed = sink.text.len();
        let seen = view.lock().expect("the view collector does not panic");
        assert_eq!(seen.deltas.len(), 20);
        assert_eq!(seen.copied, streamed);
        assert_eq!(streamed, 100);
    }

    /// A cut stream: the deltas are a prefix of the accumulation, which is what
    /// the committed entry carries, and a killed event reaches no one.
    #[test]
    fn a_cut_stream_sends_a_prefix_and_keeps_the_rest_for_the_entry() {
        let view = Arc::new(Mutex::new(View::default()));
        let mut sink = sink(Arc::clone(&view));
        stream(&mut sink, 10);
        sink.kill.store(true, Ordering::SeqCst);
        assert!(!sink.event(TurnEvent::Text("cut".into())));
        let seen = view.lock().expect("the view collector does not panic");
        let visible: String = seen.deltas.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(
            visible,
            sink.text
                .get(..visible.len())
                .expect("the deltas are a prefix of the accumulation")
        );
        assert_eq!(seen.copied, visible.len());
        assert!(!sink.text.contains("cut"));
    }

    /// Production's cadence is a real gate: the frame after an emission is
    /// silent, so growth inside one frame never reaches two deltas — the
    /// re-render ceiling ADR-0008 sets. What the window suppresses is not lost:
    /// the accumulator carries it to the entry's record.
    /// The assertion is about the window being closed, not about time: it
    /// needs no sleep, only that the harness does not preempt the two events
    /// by a whole 16 ms frame between them.

    #[test]
    fn a_frame_window_emits_one_delta() {
        let view = Arc::new(Mutex::new(View::default()));
        let mut sink = sink_at(Arc::clone(&view), STREAM_INTERVAL);
        sink.event(TurnEvent::Text("one".into()));
        sink.event(TurnEvent::Text(" two".into()));
        let seen = view.lock().expect("the view collector does not panic");
        assert_eq!(seen.deltas.len(), 1, "the frame window emitted twice");
        assert_eq!(seen.deltas[0].text, "one");
        assert_eq!(sink.text, "one two");
    }

    /// The byte marks split the accumulator where UTF-8 allows: no delta
    /// carries half a character.
    #[test]
    fn multibyte_text_splits_on_char_boundaries() {
        let view = Arc::new(Mutex::new(View::default()));
        let mut sink = sink(Arc::clone(&view));
        for word in ["héllo", "世界", " 🎉", "ok"] {
            sink.event(TurnEvent::Text(word.into()));
        }
        let seen = view.lock().expect("the view collector does not panic");
        let text: String = seen.deltas.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(text, "héllo世界 🎉ok");
        // A delta cut at a byte mark the accumulator itself produced carries a
        // whole character; the equality above is what would panic on a split.
        assert_eq!(seen.deltas.len(), 4);
    }
}
