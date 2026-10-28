//! How a provider's ending reaches the record (#307): every assistant entry
//! names how its response ended, and a cut nobody asked for says so in its own
//! entry — the entry says what survived, the diagnostic says what ended.

use super::*;

use crate::agent::testkit::*;

/// A response the provider stopped at its own output cap: the shape that made
/// a long write silently disappear (#11).
fn capped_body() -> String {
    concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"half a sentence\"}\n\n",
        "data: {\"type\":\"response.incomplete\",\"response\":",
        "{\"incomplete_details\":{\"reason\":\"max_output_tokens\"},",
        "\"usage\":{\"input_tokens\":9,\"output_tokens\":4,\"total_tokens\":13}}}\n\n",
        "data: [DONE]\n\n"
    )
    .to_owned()
}

async fn cut_turn(body: String) -> (tempfile::TempDir, Vec<Entry>) {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![body]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().store);
    (dir, entries)
}

#[tokio::test]
async fn a_capped_turn_records_the_cap_and_says_so() {
    let (_dir, entries) = cut_turn(capped_body()).await;
    assert_eq!(
        entries.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
        vec![EntryKind::User, EntryKind::Assistant, EntryKind::Diagnostic]
    );
    let assistant = &entries[1].payload;
    // The partial survives, and the record says why it is partial.
    assert_eq!(assistant["text"], "half a sentence");
    assert_eq!(assistant["stop_reason"], "length");
    assert!(
        !assistant["interrupted"].as_bool().unwrap(),
        "a cap is not a user cut"
    );
    let cutoff = &entries[2].payload;
    assert_eq!(cutoff["code"], "turn_cutoff");
    assert_eq!(cutoff["ended_by"], "length");
    assert_eq!(cutoff["model"], "test-model");
    assert_eq!(cutoff["blocks"], 1);
    // The capped call still settled: its usage is real and recorded.
    assert_eq!(assistant["usage"]["output_tokens"], 4);
}

#[tokio::test]
async fn a_clean_turn_records_its_ending_and_says_nothing() {
    let (_dir, entries) = cut_turn(sse("done", &[])).await;
    assert_eq!(
        entries.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
        vec![EntryKind::User, EntryKind::Assistant]
    );
    assert_eq!(entries[1].payload["stop_reason"], "completed");
}

#[tokio::test]
async fn a_stream_that_stops_by_itself_names_itself() {
    // Deltas, then the body ends: no terminal frame, no [DONE].
    let (_dir, entries) = cut_turn(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"par\"}\n\n".to_owned(),
    )
    .await;
    assert_eq!(
        entries.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
        vec![EntryKind::User, EntryKind::Assistant, EntryKind::Diagnostic]
    );
    assert_eq!(entries[1].payload["stop_reason"], "eof_without_done");
    let cutoff = &entries[2].payload;
    assert_eq!(cutoff["code"], "turn_cutoff");
    assert_eq!(cutoff["ended_by"], "eof_without_done");
}

#[tokio::test]
async fn every_incomplete_reason_reads_as_no_completion() {
    // A server may name a reason this build has never seen; none of them may
    // read as a clean completion, with or without the [DONE] after it (#307).
    for reason in ["", "\"content_filter\"", "\"made_up_by_a_newer_server\""] {
        for done in [true, false] {
            let details = if reason.is_empty() {
                String::new()
            } else {
                format!("\"incomplete_details\":{{\"reason\":{reason}}},")
            };
            let mut body = concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"half\"}\n\n",
                "data: {\"type\":\"response.incomplete\",\"response\":{",
            )
            .to_owned()
                + &details
                + "\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n";
            if done {
                body.push_str("data: [DONE]\n\n");
            }
            let (_dir, entries) = cut_turn(body).await;
            assert_eq!(
                entries.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
                vec![EntryKind::User, EntryKind::Assistant, EntryKind::Diagnostic],
                "reason {reason} done {done}"
            );
            assert_eq!(
                entries[1].payload["stop_reason"], "eof_without_done",
                "reason {reason} done {done}"
            );
            assert_eq!(
                entries[2].payload["ended_by"], "eof_without_done",
                "reason {reason} done {done}"
            );
        }
    }
}

#[test]
fn a_user_cut_outranks_a_provider_cut() {
    // One entry tells one cause: a stop the user asked for silences the
    // diagnostic even when the provider also cut the turn (#307).
    use crate::agent::turn::reported_cutoff;
    use tau_protocol::StreamOutcome;
    assert_eq!(reported_cutoff(StreamOutcome::Length, true), None);
    assert_eq!(
        reported_cutoff(StreamOutcome::Length, false),
        Some(StreamOutcome::Length),
        "a cap nobody asked for is a cutoff"
    );
    assert_eq!(
        reported_cutoff(StreamOutcome::MidStreamError, false),
        Some(StreamOutcome::MidStreamError)
    );
    assert_eq!(
        reported_cutoff(StreamOutcome::Completed, false),
        None,
        "a clean turn is no one's cut"
    );
    assert_eq!(
        reported_cutoff(StreamOutcome::Killed, false),
        None,
        "a stop the sink named is not a provider cut"
    );
}
