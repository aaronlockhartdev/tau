//! Mid-episode async buffering integration tests (ticket #86 P2): the
//! interval-boundary trigger, the background buffer cycle, and the
//! mid-loop activation, end to end through the harness.

use super::*;
use crate::harness::testkit::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A captured request: (system-prompt instructions, serialized input).
type SeenCalls = Vec<(Option<String>, String)>;
/// Per-call scripted provider that interleaves the main turn with the OM
/// observer round-trips: a call whose instructions are the observer's
/// system prompt serves `om` (or fails, for the exhaustion tests); every
/// other call pops the next main body. `seen` captures each request's
/// (instructions, input-JSON) for the window-bounding assertions, and
/// `main_delay_ms` models LLM latency per main call — the background
/// cycle completes inside it (a real LLM call takes seconds; the cycle's
/// is milliseconds).
struct ScriptedOm {
    main: Vec<String>,
    om: String,
    om_fail: bool,
    main_delay_ms: u64,
    index: AtomicUsize,
    served: Arc<AtomicUsize>,
    seen: Arc<Mutex<SeenCalls>>,
}

impl provider::TurnProvider for ScriptedOm {
    fn call<'a>(
        &self,
        request: &provider::ResponseRequest,
        sink: &'a mut dyn provider::TurnSink,
    ) -> provider::ProviderTurn<'a> {
        self.served.fetch_add(1, Ordering::SeqCst);
        let is_om = request
            .instructions()
            .is_some_and(|i| i == crate::om::observer_system_prompt());
        let (body, delay, failing) = if is_om {
            // The canned provider decodes SSE: wrap the observation body
            // in a delta frame, like the main-turn bodies.
            (crate::agent::testkit::sse_json(&self.om), 0, self.om_fail)
        } else {
            (
                self.main
                    .get(self.index.fetch_add(1, Ordering::SeqCst))
                    .cloned()
                    .unwrap_or_else(|| crate::agent::testkit::sse("done", &[])),
                self.main_delay_ms,
                false,
            )
        };
        let seen = (
            request.instructions().map(str::to_owned),
            serde_json::to_string(&request.input()).unwrap(),
        );
        let seen_mutex = self.seen.clone();
        let inner = provider::canned(&body);
        let turn = inner.call(request, sink);
        Box::pin(async move {
            seen_mutex.lock().unwrap().push(seen);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if failing {
                return Err(provider::ProviderError::IdleTimeout);
            }
            turn.await
        })
    }
}

fn scripted_om(
    main: Vec<String>,
    om: &str,
    om_fail: bool,
    main_delay_ms: u64,
) -> (Arc<ScriptedOm>, Arc<AtomicUsize>, Arc<Mutex<SeenCalls>>) {
    let p = Arc::new(ScriptedOm {
        main,
        om: om.to_owned(),
        om_fail,
        main_delay_ms,
        index: AtomicUsize::new(0),
        served: Arc::new(AtomicUsize::new(0)),
        seen: Arc::new(Mutex::new(Vec::new())),
    });
    (p.clone(), Arc::clone(&p.served), Arc::clone(&p.seen))
}

fn om_config(
    observe: u64,
    reflect: u64,
    increment: u64,
    buffer: u64,
    observe_retries: u32,
    buffer_retries: u32,
) -> crate::config::Om {
    crate::config::Om {
        om_model: String::new(),
        observe_threshold: observe,
        reflect_threshold: reflect,
        buffer_increment: increment,
        buffer_tokens: buffer,
        retries: crate::config::OmRetries {
            observe: observe_retries,
            buffer: buffer_retries,
            reflect: 0,
        },
    }
}

/// One tool round of known size: `task_create` echoes its title verbatim
/// (a 3200-char title is ~800 tool-result tokens).
fn tool_round(call_id: &str, title: &str) -> String {
    let args = serde_json::json!({ "title": title }).to_string();
    crate::agent::testkit::sse("", &[("task_create".to_owned(), call_id.to_owned(), args)])
}

struct Rig {
    core: Arc<Core>,
    cwd: String,
    session_id: String,
    collected: Arc<Mutex<Vec<Event>>>,
}

fn rig(om: crate::om_integration::OmState, provider: provider::TurnProviderRef) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let core = CoreBuilder::custom(providers()).build();
    let workspace = match core
        .dispatch(Command::WorkspaceOpen {
            cwd: tmp.path().to_string_lossy().into_owned(),
        })
        .unwrap()
    {
        CommandOutput::Workspace { workspace: w } => w,
        other => panic!("expected a workspace: {other:?}"),
    };
    let live = manual_session_om(&core, &workspace, provider, TurnConfig::default(), Some(om));
    let collected = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&collected);
    let pump_core = Arc::clone(&core);
    tokio::spawn(async move {
        crate::harness::pump::pump(pump_core, move |batch| {
            sink.lock().unwrap().extend(batch.iter().cloned());
        })
        .await;
    });
    // The session file must outlive the tempdir until the assertions.
    std::mem::forget(tmp);
    Rig {
        core,
        cwd: workspace.cwd.clone(),
        session_id: live.meta.lock().unwrap().id.clone(),
        collected,
    }
}

impl Rig {
    async fn run_turn(&self, text: &str) {
        self.core
            .dispatch(Command::MessageSend {
                session: self.session_id.clone(),
                text: text.to_owned(),
                lane: MessageLane::Steering,
            })
            .unwrap();
        for _ in 0..3000 {
            let done = self
                .collected
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::StreamEnd { .. }));
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    fn events(&self) -> Vec<Event> {
        self.collected.lock().unwrap().clone()
    }

    /// The session entries in file order (a fresh store over the file).
    fn entries(&self) -> Vec<crate::session::Entry> {
        let mut store = crate::session::SessionStore::for_workspace(
            std::path::Path::new(&self.cwd),
            &self.session_id,
        );
        store.open().unwrap();
        store.entries_range(0, usize::MAX).unwrap()
    }

    fn settled_cleanly(events: &[Event]) {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::EntryUpsert { entry, .. }
                    if entry.kind == "assistant")),
            "the turn's assistant entry is missing: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                Event::StreamEnd {
                    interrupted: true,
                    ..
                }
            )),
            "an OM path surfaced as an interrupted turn: {events:?}"
        );
    }
}

/// T3: one tool round below the observe threshold crosses the 200-token
/// buffer boundary; the background cycle commits ONE durable chunk and
/// the turn settles with no observe (809 < 1000) and no sync buffer
/// (the arm is gated while `buffer_tokens > 0`).
#[tokio::test]
async fn a_boundary_trigger_buffers_a_chunk_in_the_background() {
    let title = "A".repeat(3200);
    let om = crate::om_integration::OmState::from_config(
        &om_config(1000, 10_000, 400, 200, 0, 0),
        crate::om::OmRecord::default(),
    );
    let (p, served, _seen) = scripted_om(
        vec![
            tool_round("c1", &title),
            crate::agent::testkit::sse("done", &[]),
        ],
        "<observations>the work</observations>",
        false,
        50,
    );
    let rig = rig(om, p);
    rig.run_turn("go").await;
    let events = rig.events();
    Rig::settled_cleanly(&events);

    // Three provider calls: the two main-turn calls and the one cycle
    // call — no turn-end observe, no turn-end sync buffer.
    assert_eq!(served.load(Ordering::SeqCst), 3);

    // The chunk is durable in the session file, spanning the first entry
    // to the tool entry.
    let entries = rig.entries();
    let om_entry = entries
        .iter()
        .filter(|e| e.kind == "om")
        .max_by_key(|e| e.timestamp)
        .expect("an om entry is recorded");
    let record: crate::om::OmRecord =
        serde_json::from_value(om_entry.payload.clone()).expect("inline om record");
    assert_eq!(record.buffered_chunks.len(), 1, "{record:?}");
    let chunk = &record.buffered_chunks[0];
    assert_eq!(chunk.range.0, entries[0].id);
    let tool_id = entries
        .iter()
        .find(|e| e.kind == "tool")
        .expect("the tool entry")
        .id
        .clone();
    assert_eq!(chunk.range.1, tool_id);
    assert_eq!(record.last_buffered_at_tokens, 809);
    // A fresh process sees the same chunk (the record is the store).
    let mut store = crate::session::SessionStore::for_workspace(
        std::path::Path::new(&rig.cwd),
        &rig.session_id,
    );
    store.open().unwrap();
    assert_eq!(
        crate::om_integration::OmState::load_record(&mut store)
            .unwrap()
            .buffered_chunks
            .len(),
        1
    );
}

/// T4: the cycle's observer call fails and exhausts its per-trigger
/// retries: the turn settles cleanly with no chunk and an unadvanced
/// cursor — the exhaustion contract (D8) end to end.
#[tokio::test]
async fn an_exhausted_buffer_cycle_leaves_no_chunk_and_a_clean_turn() {
    let title = "A".repeat(3200);
    let om = crate::om_integration::OmState::from_config(
        &om_config(1000, 10_000, 400, 200, 0, 1),
        crate::om::OmRecord::default(),
    );
    let (p, served, _seen) = scripted_om(
        vec![
            tool_round("c1", &title),
            crate::agent::testkit::sse("done", &[]),
        ],
        "<observations>unreachable</observations>",
        true,
        50,
    );
    let rig = rig(om, p);
    rig.run_turn("go").await;
    let events = rig.events();
    Rig::settled_cleanly(&events);

    // Main 2 + the cycle's 2 attempts (1 configured retry); the turn-end
    // pass does NOT add a sync-buffer attempt of its own.
    assert_eq!(served.load(Ordering::SeqCst), 4);

    // The failed cycle committed nothing and the Done pass saves nothing: any
    // om entry in the file must be chunk-less and cursor-less.
    let entries = rig.entries();
    let om_records: Vec<crate::om::OmRecord> = entries
        .iter()
        .filter(|e| e.kind == "om")
        .filter_map(|e| serde_json::from_value::<crate::om::OmRecord>(e.payload.clone()).ok())
        .collect();
    assert!(
        om_records
            .iter()
            .all(|r| r.buffered_chunks.is_empty() && r.cursor.is_none()),
        "a failed cycle left a chunk: {om_records:?}"
    );
}

/// T6: a failing cycle with zero retries makes exactly one attempt and
/// the turn-end pass adds no sync-buffer attempt of its own — with
/// `buffer_tokens > 0` the turn-end pass never buffers (D2), even for a
/// pending (809) sitting in the old sync-buffer band [400, 1000).
#[tokio::test]
async fn the_turn_end_pass_does_not_sync_buffer_with_async_on() {
    let title = "A".repeat(3200);
    let om = crate::om_integration::OmState::from_config(
        &om_config(1000, 10_000, 400, 200, 0, 0),
        crate::om::OmRecord::default(),
    );
    let (p, served, _seen) = scripted_om(
        vec![
            tool_round("c1", &title),
            crate::agent::testkit::sse("done", &[]),
        ],
        "<observations>unreachable</observations>",
        true,
        50,
    );
    let rig = rig(om, p);
    rig.run_turn("go").await;
    let events = rig.events();
    Rig::settled_cleanly(&events);

    // Main 2 + exactly one cycle attempt (zero retries). A sync buffer at
    // the turn end would make this 4.
    assert_eq!(served.load(Ordering::SeqCst), 3);

    let entries = rig.entries();
    let om_records: Vec<crate::om::OmRecord> = entries
        .iter()
        .filter(|e| e.kind == "om")
        .filter_map(|e| serde_json::from_value::<crate::om::OmRecord>(e.payload.clone()).ok())
        .collect();
    assert!(
        om_records
            .iter()
            .all(|r| r.buffered_chunks.is_empty() && r.cursor.is_none()),
        "a failed cycle left a chunk: {om_records:?}"
    );
}

/// T5: three tool rounds (~580 tokens each, under the 600-token retention
/// floor so the raw window survives the prune) with a 1000-token observe
/// threshold: after round 1 the background cycle buffers; at round 2 the
/// pending (~1.2k) reaches the threshold and the mid-loop ACTIVATION
/// promotes the chunk (no LLM call) — so the 4th provider call's
/// assembled context carries the promoted observation, and the record
/// shows the cursor parked at round 1's tool entry.
#[tokio::test]
async fn a_threshold_reach_mid_loop_promotes_the_buffered_chunk() {
    let a = "A".repeat(2300);
    let b = "B".repeat(2300);
    let c = "C".repeat(2300);
    let om = crate::om_integration::OmState::from_config(
        &om_config(1000, 10_000, 400, 200, 0, 0),
        crate::om::OmRecord::default(),
    );
    let (p, served, seen) = scripted_om(
        vec![
            tool_round("c1", &a),
            tool_round("c2", &b),
            tool_round("c3", &c),
            crate::agent::testkit::sse("done", &[]),
        ],
        "<observations>observed the work</observations>",
        false,
        100,
    );
    let rig = rig(om, p);
    rig.run_turn("go").await;
    let events = rig.events();
    Rig::settled_cleanly(&events);

    // Main 4 + the cycle + the turn-end observe sweep (all three rounds
    // are unobserved at the end: 2.4k ≥ 1k).
    assert_eq!(served.load(Ordering::SeqCst), 6);

    // The 4th main-turn call's system prompt carries the PROMOTED
    // observation — the mid-loop activation ran before it assembled.
    let all = seen.lock().unwrap().clone();
    let main_calls: Vec<_> = all
        .iter()
        .filter(|(ins, _)| ins.as_deref() != Some(crate::om::observer_system_prompt().as_str()))
        .collect();
    assert_eq!(main_calls.len(), 4, "four main-turn calls");
    assert!(
        main_calls[3]
            .0
            .as_deref()
            .expect("main calls carry instructions")
            .contains("observed the work"),
        "the 4th call's context lacks the promoted observation"
    );

    // The promotion committed a record with the cursor at round 1's tool
    // entry (an intermediate om entry, before the turn-end sweep's).
    let entries = rig.entries();
    let tool_ids: Vec<&str> = entries
        .iter()
        .filter(|e| e.kind == "tool")
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(tool_ids.len(), 3);
    let promoted = entries
        .iter()
        .filter(|e| e.kind == "om")
        .filter_map(|e| serde_json::from_value::<crate::om::OmRecord>(e.payload.clone()).ok())
        .find(|r| r.cursor.as_ref().is_some_and(|c| c.entry_id == tool_ids[0]));
    assert!(
        promoted.is_some(),
        "no record with the cursor at round 1's tool entry: {entries:?}"
    );
    assert!(
        promoted
            .expect("checked")
            .active_observations
            .contains("observed the work")
    );
}

/// A turn whose pass plans `Done` appends no `om` entry: the write-back
/// persists only a durable chunk-state delta. The pass refreshes
/// `record.pending_tokens` on every run, so a full-record equality would
/// re-save — appending an `om` entry — on every turn of any session with
/// unobserved entries (the 10k-entry acceptance stress leg pins this).
#[tokio::test]
async fn a_done_turn_appends_no_om_entry() {
    let om = crate::om_integration::OmState::from_config(
        &om_config(1000, 10_000, 400, 200, 0, 0),
        crate::om::OmRecord::default(),
    );
    let (p, served, _seen) =
        scripted_om(vec![crate::agent::testkit::sse("done", &[])], "", false, 10);
    let rig = rig(om, p);
    rig.run_turn("go").await;
    let events = rig.events();
    Rig::settled_cleanly(&events);
    assert_eq!(served.load(Ordering::SeqCst), 1);

    let om_entries = rig.entries().iter().filter(|e| e.kind == "om").count();
    assert_eq!(om_entries, 0, "a Done turn must not append an om entry");
}
