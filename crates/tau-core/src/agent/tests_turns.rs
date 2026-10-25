use super::*;

use crate::agent::testkit::*;
use crate::provider::canned_cut;
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn a_plain_turn_appends_user_then_assistant() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("done", &[])]));
    let agent = make_agent(dir.path(), provider);
    agent.send("hello", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    assert_eq!(
        entries.iter().map(|e| e.kind.clone()).collect::<Vec<_>>(),
        vec![EntryKind::User, EntryKind::Assistant]
    );
    assert_eq!(entries[0].payload["text"], "hello");
    assert_eq!(entries[1].payload["text"], "done");
    assert!(!entries[1].payload["interrupted"].as_bool().unwrap());
}

#[tokio::test]
async fn tool_calls_roundtrip_through_the_session() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("n.txt"), "a\n").unwrap();
    let body1 = sse(
        "",
        &[("read".into(), "c1".into(), r#"{"path":"n.txt"}"#.into())],
    );
    let body2 = sse("read it", &[]);
    let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
    let agent = make_agent(dir.path(), provider);
    agent.send("read n.txt", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind.clone()).collect();
    // One tool entry per call (ADR-0008): the call's line carries the
    // result's output.
    assert_eq!(
        kinds,
        vec![
            EntryKind::User,
            EntryKind::Assistant,
            EntryKind::Tool,
            EntryKind::Assistant
        ]
    );
    assert_eq!(entries[2].payload["call_id"], "c1");
    assert_eq!(entries[2].payload["name"], "read");
    assert!(entries[2].payload["output"].as_str().unwrap().contains('a'));
}

#[tokio::test]
async fn steering_lands_on_the_next_llm_call() {
    let dir = tempfile::tempdir().unwrap();
    let body1 = sse(
        "",
        &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
    );
    let body2 = sse("after steering", &[]);
    let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
    let agent = make_agent(dir.path(), provider);
    agent.send("start", Lane::FollowUp);
    agent.send("steer me", Lane::Steering);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    // The steering message rides the next LLM call: it is a user entry
    // delivered at the head of the same turn as its starter.
    let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            EntryKind::User,
            EntryKind::User,
            EntryKind::Assistant,
            EntryKind::Tool,
            EntryKind::Assistant
        ]
    );
    assert_eq!(entries[1].payload["text"], "steer me");
    assert_eq!(entries[1].payload["lane"], "steering");
}

#[tokio::test]
async fn steering_consumption_fires_the_queue_hook() {
    let dir = tempfile::tempdir().unwrap();
    let body1 = sse(
        "",
        &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
    );
    let body2 = sse("after steering", &[]);
    let provider = Arc::new(ScriptedProvider::new(vec![body1, body2]));
    let agent = make_agent(dir.path(), provider);
    // A queue-change hook: fired when a steering/force message is consumed
    // mid-turn, so the app can re-emit the queue snapshot now (not at the
    // turn boundary).
    let fired = Arc::new(AtomicUsize::new(0));
    let fired_clone = fired.clone();
    agent.set_queue_event_hook(Some(Arc::new(move || {
        fired_clone.fetch_add(1, Ordering::SeqCst);
    })));
    agent.send("start", Lane::FollowUp);
    agent.send("steer me", Lane::Steering);
    agent.process().await.unwrap();
    // The mid-turn steering consumption fired the hook.
    assert!(fired.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn follow_up_lands_after_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("first done", &[]),
        sse("second done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("first", Lane::FollowUp);
    agent.send("later", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            EntryKind::User,
            EntryKind::Assistant,
            EntryKind::User,
            EntryKind::Assistant
        ]
    );
    assert_eq!(entries[1].payload["text"], "first done");
    // The follow-up starts the *next* turn: its user entry sits between
    // the two assistant entries, not inside the first turn.
    assert_eq!(entries[2].payload["text"], "later");
    assert_eq!(entries[3].payload["text"], "second done");
}

#[tokio::test]
async fn force_kills_the_stream_and_keeps_the_partial() {
    let dir = tempfile::tempdir().unwrap();
    // A long stream the force cuts after two text events; no calls, so
    // the turn ends at the kill.
    let body = sse("a", &[]) + sse("b", &[]).as_str();
    let provider = canned_cut(&body, 1);
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let assistant = entries
        .iter()
        .find(|e| e.kind == EntryKind::Assistant)
        .unwrap();
    // A transport cut (the stream dropped, no user stop) is not an
    // interruption -- only a user stop/force/close is.
    assert!(!assistant.payload["interrupted"].as_bool().unwrap());
    assert_eq!(assistant.payload["text"], "a");
}
#[tokio::test]
async fn a_tool_call_response_that_omits_completed_is_not_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    // A tool-call response whose server omits the response.completed frame
    // (some do, on tool-call turns) and ends cleanly with [DONE]: it is a
    // complete, normal agentic segment, not an interrupted one. Regression:
    // `interrupted = !completed` flagged every such segment as interrupted.
    let item = serde_json::json!({
        "id": "c1",
        "type": "function_call",
        "name": "bash",
        "call_id": "c1",
        "arguments": "{\"command\":\"true\"}",
    });
    let tool_call_no_completed = format!(
        "data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\ndata: [DONE]\n\n"
    );
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_call_no_completed,
        sse("done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let assistants: Vec<_> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Assistant)
        .collect();
    assert_eq!(assistants.len(), 2, "tool-call segment + final segment");
    for a in &assistants {
        assert!(
            a.payload["interrupted"].as_bool() == Some(false),
            "segment must not be interrupted: {:?}",
            a.payload
        );
    }
}

#[tokio::test]
async fn force_preempts_a_queued_steering_at_the_head_of_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse("killed", &[]),
        sse("next", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.send("queued steering", Lane::Steering);
    // The force arrives mid-turn: kill + head of queue.
    agent.send("FORCE", Lane::Force);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let users: Vec<&str> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::User)
        .map(|e| e.payload["text"].as_str().unwrap())
        .collect();
    // The forced message is delivered before the queued steering, on
    // the turn that follows the killed one.
    assert_eq!(users, vec!["FORCE", "queued steering", "go"]);
}

/// A force sent while the stream is IN FLIGHT (not a pre-cut): the kill
/// flag stops the slow stream mid-way, the partial stands as interrupted,
/// and the forced message preempts a steering queued at the same instant
/// on the next call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_mid_stream_kills_the_stream_and_preempts_contemporaneous_steering() {
    let dir = tempfile::tempdir().unwrap();
    // One stream, four 40 ms-apart text deltas, ending in a single
    // completed event: left alone it runs ~200 ms to completion, the
    // force at ~100 ms cuts it mid-way. (Built by hand — sse() ends each
    // chunk in [DONE], and the decoder stops at the first one.)
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"c\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"d\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        "data: [DONE]\n\n"
    );
    let provider = crate::provider::canned_slow(body, 40);
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    let killer = {
        let agent = agent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            agent.send("FORCE", Lane::Force);
            agent.send("steer", Lane::Steering);
        })
    };
    agent.process().await.unwrap();
    killer.await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            EntryKind::User,
            EntryKind::Assistant,
            EntryKind::User,
            EntryKind::User,
            EntryKind::Assistant
        ]
    );
    let stopped = &entries[1];
    assert!(
        stopped.payload["interrupted"].as_bool().unwrap(),
        "{stopped:?}"
    );
    // a, b, c land before the 100 ms force (d is at 120 ms); under a
    // loaded runner the 80 ms c may lose the race, so accept ab or abc.
    let partial = stopped.payload["text"].as_str().unwrap();
    assert!(
        partial == "ab" || partial == "abc",
        "partial was {partial:?}"
    );
    assert_eq!(entries[2].payload["text"], "FORCE");
    assert_eq!(entries[3].payload["text"], "steer");
    // The following call starts un-killed and runs the stream to completion.
    assert_eq!(entries[4].payload["text"], "abcd");
    assert!(!entries[4].payload["interrupted"].as_bool().unwrap());
}

#[tokio::test]
async fn append_propagates_storage_errors_not_a_new_root() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![sse("ok", &[])]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    // Lose the file under the session: the next append must fail with a
    // storage error, not silently start a new root branch.
    std::fs::remove_file(agent.inner.lock().unwrap().store.path()).unwrap();
    agent.send("again", Lane::FollowUp);
    assert!(agent.process().await.is_err());
}

#[tokio::test]
async fn kill_policy_completes_the_inflight_tool_batch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.txt"), "x\n").unwrap();
    // The killed stream had a completed function_call before the cut:
    // with the Complete policy the tool runs and the turn continues to
    // a final call where the forced message lands alongside the result.
    let cut = "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"c1\",\"type\":\"function_call\",\"name\":\"read\",\"call_id\":\"c1\",\"arguments\":\"{\\\"path\\\":\\\"x.txt\\\"}\"}}\n\n";
    let provider = Arc::new(ScriptedProvider::new(vec![
        cut.to_string(),
        sse("done", &[]),
    ]));
    let agent = AgentSession::new(SessionParams {
        store: session_in(dir.path()),
        system_prompt: "be terse".into(),
        model: "test-model".into(),
        tools: tools::tool_specs(),
        cwd: dir.path().to_path_buf(),
        provider,
        tool_batch_on_force: ToolBatchPolicy::Complete,
        turn: TurnConfig::default(),
        gates: None,
        gate_failed: false,
        hashline: crate::hashline::pin_store::GcPolicy::default(),
        om: None,
        om_model: String::new(),
        subagents: None,
        child: None,
    });
    agent.send("go", Lane::FollowUp);
    agent.send("FORCE", Lane::Force);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind.clone()).collect();
    // killed assistant + tool result + the final call, whose prompt head
    // carries the forced user entry alongside the tool result.
    assert!(kinds.contains(&EntryKind::Tool), "{kinds:?}");
    let interrupted = entries
        .iter()
        .find(|e| {
            e.kind == EntryKind::Assistant && e.payload["interrupted"].as_bool() == Some(true)
        })
        .unwrap();
    assert_eq!(interrupted.payload["calls"].as_array().unwrap().len(), 1);
}

/// ticket #70: a tool call whose arguments are not valid JSON and cannot be
/// repaired yields a parse-error tool result — never a silent null-args
/// dispatch that would surface as a confusing schema error.
#[tokio::test]
async fn unrepairable_tool_args_yield_a_parse_error_not_a_null() {
    let dir = tempfile::tempdir().unwrap();
    // A bash call whose arguments are not valid JSON (unrepairable).
    let agent = make_agent(
        dir.path(),
        Arc::new(ScriptedProvider::new(vec![sse(
            "",
            &[("bash".into(), "call-1".into(), "not valid json {[".into())],
        )])),
    );
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();

    // Read the tool-result entry back off the session file.
    let store = SessionStore::for_workspace(dir.path(), "s1");
    let entries = entries_of(&store);
    let tool = entries
        .iter()
        .find(|e| e.kind == EntryKind::Tool)
        .expect("expected a tool result entry");
    let output = tool
        .payload
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        output.contains("not valid JSON"),
        "the tool result should carry the parse error, got: {output:?}"
    );
}

/// #186: two `edit` calls on the same file in one batch — both are refused
/// pre-write (they would share one stale anchor snapshot); the file stays
/// untouched.
#[tokio::test]
async fn parallel_edits_to_the_same_file_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dup.txt"), "one\ntwo\n").unwrap();
    let args1 =
        "{\"path\":\"dup.txt\",\"edits\":[{\"from\":\"aaa\",\"to\":\"aaa\",\"content\":\"ONE\"}]}";
    let args2 =
        "{\"path\":\"dup.txt\",\"edits\":[{\"from\":\"bbb\",\"to\":\"bbb\",\"content\":\"TWO\"}]}";
    let body = sse(
        "",
        &[
            ("edit".into(), "c1".into(), args1.into()),
            ("edit".into(), "c2".into(), args2.into()),
        ],
    );
    let agent = make_agent(
        dir.path(),
        Arc::new(ScriptedProvider::new(vec![body, sse("done", &[])])),
    );
    agent.send("go", Lane::FollowUp);
    agent.process().await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let tools: Vec<&Value> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Tool)
        .map(|e| &e.payload)
        .collect();
    assert_eq!(tools.len(), 2, "one entry per call");
    for call_id in ["c1", "c2"] {
        let rejected = tools
            .iter()
            .copied()
            .find(|p| p["call_id"] == call_id)
            .unwrap_or(&Value::Null);
        assert!(
            rejected["output"]
                .as_str()
                .is_some_and(|o| o.contains("already edited in this batch")),
            "{call_id} must be refused pre-write: {rejected:?}"
        );
        assert_eq!(rejected["status"], "error");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("dup.txt")).unwrap(),
        "one\ntwo\n",
        "the file must be untouched"
    );
}

// ticket #222: a stop during an in-flight tool batch cancels the tools.

#[tokio::test]
async fn stop_cancels_an_inflight_bash() {
    let dir = tempfile::tempdir().unwrap();
    // A 3 s bash: the stop lands mid-command, the process is killed within a
    // bounded time, and the tool entry is marked cancelled (not a hung
    // pending state, not a plain error).
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse(
            "",
            &[(
                "bash".into(),
                "c1".into(),
                r#"{"command":"sleep 3"}"#.into(),
            )],
        ),
        sse("done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    let stopper = {
        let agent = agent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            agent.stop();
        })
    };
    let started = std::time::Instant::now();
    agent.process().await.unwrap();
    stopper.await.unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "stop did not bound the in-flight bash: {elapsed:?}"
    );
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let tool = entries.iter().find(|e| e.kind == EntryKind::Tool).unwrap();
    assert_eq!(tool.payload["status"], "cancelled", "{tool:?}");
    assert!(
        tool.payload["output"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
    // The turn is cut after the cancelled batch: the second (scripted)
    // provider call is killed by the sink before it can emit anything, so no
    // further assistant segment follows the tool entry.
    let kinds: Vec<EntryKind> = entries.iter().map(|e| e.kind.clone()).collect();
    let tool_pos = kinds.iter().position(|k| *k == EntryKind::Tool).unwrap();
    assert!(
        !kinds
            .iter()
            .skip(tool_pos + 1)
            .any(|k| *k == EntryKind::Assistant),
        "no assistant segment after the cancelled tool: {kinds:?}"
    );
}

#[tokio::test]
async fn stop_cancels_the_rest_of_the_tool_batch() {
    let dir = tempfile::tempdir().unwrap();
    // A batch of two bash calls: the first finishes (its marker file lands),
    // the stop lands while the second runs — it is cancelled, not started.
    let marker = dir.path().join("marker");
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse(
            "",
            &[
                (
                    "bash".into(),
                    "c1".into(),
                    format!(r#"{{"command":"echo a > {}"}}"#, marker.display()),
                ),
                (
                    "bash".into(),
                    "c2".into(),
                    r#"{"command":"sleep 3"}"#.into(),
                ),
            ],
        ),
        sse("done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    let stopper = {
        let agent = agent.clone();
        tokio::spawn(async move {
            // Poll for the first tool's marker: the stop lands once the
            // second dispatch has begun, not in the gap between provider
            // calls.
            while !marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            agent.stop();
        })
    };
    let started = std::time::Instant::now();
    agent.process().await.unwrap();
    stopper.await.unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "stop did not bound the batch: {elapsed:?}"
    );
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let tools: Vec<&Value> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Tool)
        .map(|e| &e.payload)
        .collect();
    assert_eq!(tools.len(), 2, "one entry per call");
    // The first call ran to completion before the stop: it stands as ok.
    let first = tools
        .iter()
        .copied()
        .find(|p| p["call_id"] == "c1")
        .expect("first call entry");
    assert_eq!(first["status"], "ok", "{first:?}");
    // The second call is cancelled without executing.
    let second = tools
        .iter()
        .copied()
        .find(|p| p["call_id"] == "c2")
        .expect("second call entry");
    assert_eq!(second["status"], "cancelled", "{second:?}");
    assert!(second["output"].as_str().unwrap().contains("cancelled"));
}

#[tokio::test]
async fn double_stop_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse(
            "",
            &[(
                "bash".into(),
                "c1".into(),
                r#"{"command":"sleep 3"}"#.into(),
            )],
        ),
        sse("done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    let stopper = {
        let agent = agent.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            agent.stop();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            agent.stop();
        })
    };
    let started = std::time::Instant::now();
    agent.process().await.unwrap();
    stopper.await.unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "double stop must not stall the turn: {elapsed:?}"
    );
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let tools: Vec<&Value> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Tool)
        .map(|e| &e.payload)
        .collect();
    assert_eq!(tools.len(), 1, "exactly one tool entry");
    assert_eq!(tools[0]["status"], "cancelled");
}

#[tokio::test]
async fn a_stop_after_the_tool_finished_is_not_a_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    // The tool runs to completion well before the stop arrives: the result
    // stands as ok, not a spurious cancellation.
    let provider = Arc::new(ScriptedProvider::new(vec![
        sse(
            "",
            &[("bash".into(), "c1".into(), r#"{"command":"true"}"#.into())],
        ),
        sse("done", &[]),
    ]));
    let agent = make_agent(dir.path(), provider);
    agent.send("go", Lane::FollowUp);
    let stopper = {
        let agent = agent.clone();
        tokio::spawn(async move {
            // Well after the ~100 ms turn has finished.
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            agent.stop();
        })
    };
    agent.process().await.unwrap();
    stopper.await.unwrap();
    let entries = entries_of(&agent.inner.lock().unwrap().store);
    let tool = entries.iter().find(|e| e.kind == EntryKind::Tool).unwrap();
    assert_eq!(tool.payload["status"], "ok", "{tool:?}");
    assert!(
        !tool.payload["output"]
            .as_str()
            .unwrap()
            .contains("cancelled")
    );
}
